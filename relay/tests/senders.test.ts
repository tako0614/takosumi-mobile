import { expect, test } from "bun:test";
import { base64UrlDecode, toArrayBuffer, utf8 } from "../src/encoding.ts";
import { APNS_CONFIGURATION_REASONS, APNS_HOSTS, ApnsSender, apnsPayload } from "../src/senders/apns.ts";
import { FCM_SCOPE, FcmSender, GOOGLE_TOKEN_URL, fcmMessage, parseServiceAccount } from "../src/senders/fcm.ts";
import { MAX_PUSH_BODY_BYTES, type DeliveryOutcome, type DeliveryRequest } from "../src/senders/types.ts";

const NOW = 1_800_000_000;

function pem(der: ArrayBuffer): string {
  const base64 = btoa(String.fromCharCode(...new Uint8Array(der)));
  return `-----BEGIN PRIVATE KEY-----\n${base64.match(/.{1,64}/gu)?.join("\n")}\n-----END PRIVATE KEY-----\n`;
}

async function apnsKey() {
  const pair = (await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"])) as CryptoKeyPair;
  return { pem: pem(await crypto.subtle.exportKey("pkcs8", pair.privateKey)), publicKey: pair.publicKey };
}

async function serviceAccountKey() {
  const pair = (await crypto.subtle.generateKey(
    { name: "RSASSA-PKCS1-v1_5", modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: "SHA-256" },
    true,
    ["sign", "verify"],
  )) as CryptoKeyPair;
  return { pem: pem(await crypto.subtle.exportKey("pkcs8", pair.privateKey)), publicKey: pair.publicKey };
}

function decodeJwt(token: string) {
  const [h = "", c = "", s = ""] = token.split(".");
  const json = (segment: string) => JSON.parse(new TextDecoder().decode(base64UrlDecode(segment) ?? new Uint8Array()));
  return { header: json(h), claims: json(c), signature: base64UrlDecode(s) ?? new Uint8Array(), signingInput: `${h}.${c}` };
}

function request(overrides: Partial<DeliveryRequest> = {}): DeliveryRequest {
  return {
    platform: "apns",
    token: "ab".repeat(32),
    apnsEnvironment: "production",
    subscriptionId: "SUBSCRIPTIONid_0123456",
    body: new Uint8Array([1, 2, 3, 250, 251, 252]),
    ttl: 3600,
    urgency: "normal",
    topic: null,
    now: NOW,
    ...overrides,
  };
}

interface Captured {
  url: string;
  init: RequestInit;
}

function mockFetch(responses: (() => Response)[]) {
  const calls: Captured[] = [];
  const fetcher = async (url: string, init: RequestInit = {}) => {
    calls.push({ url, init });
    const next = responses.shift();
    if (!next) throw new Error("unexpected fetch");
    return next();
  };
  return { calls, fetcher };
}

const header = (call: Captured | undefined, name: string) => (call?.init.headers as Record<string, string>)[name];

test("APNs: sends an alert with mutable-content, placeholder text and only {s, b}", async () => {
  const key = await apnsKey();
  const { calls, fetcher } = mockFetch([() => new Response(null, { status: 200 })]);
  const sender = new ApnsSender({ teamId: "TEAM123456", keyId: "KEY1234567", privateKeyPem: key.pem, topic: "com.takosumi" }, fetcher);
  expect(await sender.send(request({ topic: "room-1" }))).toEqual({ kind: "delivered" });

  const call = calls[0];
  expect(call?.url).toBe(`${APNS_HOSTS.production}/3/device/${"ab".repeat(32)}`);
  expect(header(call, "apns-push-type")).toBe("alert");
  expect(header(call, "apns-topic")).toBe("com.takosumi");
  expect(header(call, "apns-priority")).toBe("10");
  expect(header(call, "apns-expiration")).toBe(String(NOW + 3600));
  expect(header(call, "apns-collapse-id")).toBe("room-1");
  const payload = JSON.parse(String(call?.init.body));
  expect(payload).toEqual({
    aps: { alert: { title: "Takosumi", body: "新しい通知" }, "mutable-content": 1 },
    s: "SUBSCRIPTIONid_0123456",
    b: "AQID-vv8",
  });

  const jwt = decodeJwt(String(header(call, "authorization")).replace(/^bearer /u, ""));
  expect(jwt.header).toEqual({ alg: "ES256", kid: "KEY1234567" });
  expect(jwt.claims.iss).toBe("TEAM123456");
  expect(NOW - jwt.claims.iat).toBeLessThan(40 * 60);
  expect(
    await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, key.publicKey, toArrayBuffer(jwt.signature), toArrayBuffer(utf8(jwt.signingInput))),
  ).toBe(true);
});

test("APNs: priority 5 below normal, sandbox host for development, expiration 0 for TTL 0", async () => {
  const key = await apnsKey();
  const { calls, fetcher } = mockFetch([() => new Response(null, { status: 200 }), () => new Response(null, { status: 200 })]);
  const sender = new ApnsSender({ teamId: "T", keyId: "K", privateKeyPem: key.pem, topic: "com.takosumi" }, fetcher);
  await sender.send(request({ urgency: "low", apnsEnvironment: "development", ttl: 0 }));
  await sender.send(request({ urgency: "high", now: NOW + 60 }));
  expect(calls[0]?.url.startsWith(APNS_HOSTS.development)).toBe(true);
  expect(header(calls[0], "apns-priority")).toBe("5");
  expect(header(calls[0], "apns-expiration")).toBe("0");
  expect(header(calls[0], "apns-collapse-id")).toBeUndefined();
  expect(header(calls[1], "apns-priority")).toBe("10");
  // The provider token is reused within its 40-minute bucket.
  expect(header(calls[1], "authorization")).toBe(header(calls[0], "authorization"));
});

test("APNs: maps Apple's answers to relay outcomes", async () => {
  const key = await apnsKey();
  const answer = (status: number, reason?: string) => () =>
    new Response(reason ? JSON.stringify({ reason }) : null, { status, headers: { "apns-id": "x" } });
  const cases: [() => Response, DeliveryOutcome["kind"]][] = [
    [answer(410, "Unregistered"), "device-invalid"],
    [answer(410, "ExpiredToken"), "device-invalid"],
    [answer(410), "device-invalid"],
    [answer(400, "BadDeviceToken"), "device-invalid"],
    // Configuration of the relay, not of the device: never invalidate.
    [answer(400, "DeviceTokenNotForTopic"), "failed"],
    [answer(400, "TopicDisallowed"), "failed"],
    [answer(400, "MissingTopic"), "failed"],
    [answer(400, "BadTopic"), "failed"],
    [answer(403, "InvalidProviderToken"), "failed"],
    [answer(403, "ExpiredProviderToken"), "failed"],
    [answer(403, "MissingProviderToken"), "failed"],
    [answer(403, "BadEnvironmentKeyInToken"), "failed"],
    [answer(403, "Forbidden"), "failed"],
    [answer(400, "SomethingNew"), "failed"],
    [answer(400, "PayloadTooLarge"), "failed"],
    [answer(429, "TooManyRequests"), "throttled"],
    [answer(503, "ServiceUnavailable"), "unavailable"],
    [answer(500, "InternalServerError"), "unavailable"],
  ];
  for (const [respond, kind] of cases) {
    const { fetcher } = mockFetch([respond]);
    const sender = new ApnsSender({ teamId: "T", keyId: "K", privateKeyPem: key.pem, topic: "com.takosumi" }, fetcher);
    expect((await sender.send(request())).kind).toBe(kind);
  }
  const unreachable = new ApnsSender({ teamId: "T", keyId: "K", privateKeyPem: key.pem, topic: "t" }, async () => {
    throw new TypeError("Network connection lost.");
  });
  expect(await unreachable.send(request())).toEqual({ kind: "unavailable", retryAfter: null, reason: "apns-unreachable" });
  const badKey = new ApnsSender({ teamId: "T", keyId: "K", privateKeyPem: "not a key", topic: "t" }, mockFetch([]).fetcher);
  expect(await badKey.send(request())).toEqual({ kind: "failed", reason: "apns-key-unusable" });
});

test("APNs: no configuration reason can invalidate a device, whatever the status", async () => {
  const key = await apnsKey();
  for (const reason of APNS_CONFIGURATION_REASONS) {
    for (const status of [400, 403, 410]) {
      const { fetcher } = mockFetch([() => Response.json({ reason }, { status })]);
      const sender = new ApnsSender({ teamId: "T", keyId: "K", privateKeyPem: key.pem, topic: "t" }, fetcher);
      expect(`${reason}/${status}:${(await sender.send(request())).kind}`).toBe(`${reason}/${status}:failed`);
    }
  }
});

test("APNs and FCM payloads for the largest accepted body fit in 4096 bytes", () => {
  const largest = request({ body: new Uint8Array(MAX_PUSH_BODY_BYTES).fill(255), subscriptionId: "A".repeat(22), topic: "t".repeat(32) });
  const apns = utf8(JSON.stringify(apnsPayload(largest, {}))).length;
  expect(apns).toBeLessThanOrEqual(4096);
  const data = fcmMessage({ ...largest, platform: "fcm" }).message.data;
  const fcmDataBytes = Object.entries(data).reduce((sum, [k, v]) => sum + utf8(k).length + utf8(v).length, 0);
  expect(fcmDataBytes).toBeLessThanOrEqual(4096);
});

test("FCM: exchanges a service-account JWT and sends a data message with only {s, b}", async () => {
  const key = await serviceAccountKey();
  const { calls, fetcher } = mockFetch([
    () => Response.json({ access_token: "ya29.test-token", expires_in: 3599 }),
    () => Response.json({ name: "projects/p/messages/1" }),
    () => Response.json({ name: "projects/p/messages/2" }),
  ]);
  const sender = new FcmSender(
    { projectId: "takosumi-mobile", serviceAccount: { client_email: "relay@takosumi-mobile.iam.gserviceaccount.com", private_key: key.pem } },
    fetcher,
  );
  expect(await sender.send(request({ platform: "fcm", token: "fcm-token", apnsEnvironment: null, urgency: "high", topic: "room-1", ttl: 120 }))).toEqual({
    kind: "delivered",
  });
  expect(await sender.send(request({ platform: "fcm", token: "fcm-token", apnsEnvironment: null, urgency: "normal" }))).toEqual({
    kind: "delivered",
  });

  expect(calls[0]?.url).toBe(GOOGLE_TOKEN_URL);
  const form = new URLSearchParams(String(calls[0]?.init.body));
  expect(form.get("grant_type")).toBe("urn:ietf:params:oauth:grant-type:jwt-bearer");
  const assertion = decodeJwt(form.get("assertion") ?? "");
  expect(assertion.header).toEqual({ alg: "RS256", typ: "JWT" });
  expect(assertion.claims).toEqual({
    iss: "relay@takosumi-mobile.iam.gserviceaccount.com",
    scope: FCM_SCOPE,
    aud: GOOGLE_TOKEN_URL,
    iat: NOW,
    exp: NOW + 3600,
  });
  expect(
    await crypto.subtle.verify("RSASSA-PKCS1-v1_5", key.publicKey, toArrayBuffer(assertion.signature), toArrayBuffer(utf8(assertion.signingInput))),
  ).toBe(true);

  expect(calls[1]?.url).toBe("https://fcm.googleapis.com/v1/projects/takosumi-mobile/messages:send");
  expect(header(calls[1], "authorization")).toBe("Bearer ya29.test-token");
  expect(JSON.parse(String(calls[1]?.init.body))).toEqual({
    message: {
      token: "fcm-token",
      data: { s: "SUBSCRIPTIONid_0123456", b: "AQID-vv8" },
      android: { priority: "HIGH", ttl: "120s", collapse_key: "room-1" },
    },
  });
  // The access token is cached; the second message needs no token exchange.
  expect(calls).toHaveLength(3);
  expect(JSON.parse(String(calls[2]?.init.body)).message.android).toEqual({ priority: "NORMAL", ttl: "3600s" });
});

test("FCM: maps Google's answers to relay outcomes", async () => {
  const key = await serviceAccountKey();
  const error = (status: number, errorCode: string | null, message = "x", headers: Record<string, string> = {}) => () =>
    Response.json(
      {
        error: {
          code: status,
          message,
          status: errorCode ?? "UNKNOWN",
          details: errorCode ? [{ "@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError", errorCode }] : [],
        },
      },
      { status, headers },
    );
  const cases: [() => Response, DeliveryOutcome["kind"]][] = [
    [error(404, "UNREGISTERED"), "device-invalid"],
    [error(400, "INVALID_ARGUMENT", "The registration token is not a valid FCM registration token"), "device-invalid"],
    // Configuration of the relay (wrong project, credentials, permissions).
    [error(403, "SENDER_ID_MISMATCH"), "failed"],
    [error(403, "PERMISSION_DENIED"), "failed"],
    [error(404, null, "Requested entity was not found."), "failed"],
    [error(400, "INVALID_ARGUMENT", "Invalid value at 'message.data'"), "failed"],
    [error(429, "QUOTA_EXCEEDED", "x", { "retry-after": "30" }), "throttled"],
    [error(503, "UNAVAILABLE"), "unavailable"],
    [error(500, "INTERNAL"), "unavailable"],
    [error(401, "THIRD_PARTY_AUTH_ERROR"), "failed"],
  ];
  for (const [respond, kind] of cases) {
    const { fetcher } = mockFetch([() => Response.json({ access_token: "t", expires_in: 3600 }), respond]);
    const sender = new FcmSender({ projectId: "p", serviceAccount: { client_email: "e", private_key: key.pem } }, fetcher);
    const outcome = await sender.send(request({ platform: "fcm", token: "fcm-token", apnsEnvironment: null }));
    expect(outcome.kind).toBe(kind);
    if (outcome.kind === "throttled") expect(outcome.retryAfter).toBe(30);
  }
  const { fetcher } = mockFetch([() => new Response("nope", { status: 400 })]);
  const rejected = new FcmSender({ projectId: "p", serviceAccount: { client_email: "e", private_key: key.pem } }, fetcher);
  expect(await rejected.send(request({ platform: "fcm" }))).toEqual({ kind: "failed", reason: "google-oauth-400" });
});

test("FCM: reads a service account JSON and rejects anything else", () => {
  expect(parseServiceAccount(JSON.stringify({ type: "service_account", client_email: "a@b", private_key: "k" }))).toEqual({
    client_email: "a@b",
    private_key: "k",
  });
  expect(parseServiceAccount("{}")).toBeNull();
  expect(parseServiceAccount("not json")).toBeNull();
});

test("invalidations say whether the token expired or was rejected", async () => {
  const key = await apnsKey();
  const apnsCases: [number, string | undefined, string][] = [
    [410, "Unregistered", "expired"],
    [410, "ExpiredToken", "expired"],
    [410, undefined, "expired"],
    [400, "BadDeviceToken", "rejected"],
  ];
  for (const [status, reason, evidence] of apnsCases) {
    const { fetcher } = mockFetch([() => new Response(reason ? JSON.stringify({ reason }) : null, { status })]);
    const sender = new ApnsSender({ teamId: "T", keyId: "K", privateKeyPem: key.pem, topic: "t" }, fetcher);
    expect(await sender.send(request())).toMatchObject({ kind: "device-invalid", evidence });
  }
  const account = await serviceAccountKey();
  const fcmCases: [number, string, string, string][] = [
    [404, "UNREGISTERED", "Requested entity was not found.", "expired"],
    [400, "INVALID_ARGUMENT", "The registration token is not a valid FCM registration token", "rejected"],
  ];
  for (const [status, errorCode, message, evidence] of fcmCases) {
    const { fetcher } = mockFetch([
      () => Response.json({ access_token: "t", expires_in: 3600 }),
      () => Response.json({ error: { code: status, message, status: errorCode, details: [{ errorCode }] } }, { status }),
    ]);
    const sender = new FcmSender({ projectId: "p", serviceAccount: { client_email: "e", private_key: account.pem } }, fetcher);
    expect(await sender.send(request({ platform: "fcm" }))).toMatchObject({ kind: "device-invalid", evidence });
  }
});
