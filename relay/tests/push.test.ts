import { expect, test } from "bun:test";
import { base64UrlDecode, base64UrlEncode, utf8 } from "../src/encoding.ts";
import { MAX_PUSH_BODY_BYTES } from "../src/senders/types.ts";
import { decryptWebPush, encryptWebPush, generateVapidKeys } from "../tools/web-push.ts";
import {
  APNS_TOKEN,
  FCM_TOKEN,
  ORIGIN,
  SECRET_PAYLOAD,
  VAPID_SUBJECT,
  createTestRelay,
  push,
  pushInit,
  register,
  subscribe,
  type Subscribed,
} from "./support/relay.ts";

async function ready(platform: "apns" | "fcm" = "fcm", limits = {}) {
  const t = createTestRelay({ limits });
  const device = await register(
    t,
    platform === "apns" ? { platform, token: APNS_TOKEN, apns_environment: "development" } : { platform, token: FCM_TOKEN },
  );
  const subscription = await subscribe(t, device);
  return { t, device, subscription };
}

test("accepts a standard Web Push message with 201, Location and the applied TTL", async () => {
  const { t, subscription } = await ready();
  const response = await push(t, subscription, { headers: { ttl: "86400", urgency: "high", topic: "room-42" } });
  expect(response.status).toBe(201);
  expect(response.headers.get("location")).toMatch(new RegExp(`^${ORIGIN}/message/[A-Za-z0-9_-]{22}$`, "u"));
  expect(response.headers.get("ttl")).toBe("86400");

  const sent = t.fcm.requests[0];
  expect(sent).toMatchObject({
    platform: "fcm",
    token: FCM_TOKEN,
    subscriptionId: subscription.subscriptionId,
    ttl: 86400,
    urgency: "high",
    topic: "room-42",
  });
  // The body reaches the device byte for byte and still decrypts there.
  const plaintext = await decryptWebPush({
    uaPrivate: subscription.userAgent.privateKey,
    uaPublic: subscription.userAgent.publicKey,
    authSecret: subscription.userAgent.authSecret,
    body: sent?.body ?? new Uint8Array(),
  });
  expect(new TextDecoder().decode(plaintext)).toBe(SECRET_PAYLOAD);
});

test("routes APNs devices to the APNs sender with their environment", async () => {
  const { t, subscription } = await ready("apns");
  expect((await push(t, subscription)).status).toBe(201);
  expect(t.apns.requests[0]).toMatchObject({ platform: "apns", token: APNS_TOKEN, apnsEnvironment: "development", urgency: "normal", topic: null });
  expect(t.fcm.requests).toHaveLength(0);
});

test("reduces an over-long TTL and says so", async () => {
  const { t, subscription } = await ready();
  const response = await push(t, subscription, { headers: { ttl: "99999999" } });
  expect(response.status).toBe(201);
  expect(response.headers.get("ttl")).toBe(String(28 * 24 * 3600));
  expect(t.fcm.requests[0]?.ttl).toBe(28 * 24 * 3600);
});

test("answers 404 for an endpoint it never issued", async () => {
  const { t, subscription } = await ready();
  const init = await pushInit(t, subscription);
  const response = await t.call("POST", `/push/${base64UrlEncode(new Uint8Array(32))}`, init);
  expect(response.status).toBe(404);
  expect(t.fcm.requests).toHaveLength(0);
});

test("requires VAPID: 401 without it, 403 for another key, origin or expiry", async () => {
  const { t, subscription } = await ready();
  const missing = await push(t, subscription, { headers: { authorization: null } });
  expect(missing.status).toBe(401);
  expect(missing.headers.get("www-authenticate")).toBe("vapid");

  const wrongKey = await push(t, subscription, { vapid: await generateVapidKeys() });
  expect(wrongKey.status).toBe(403);
  const wrongAudience = await push(t, subscription, { audience: "https://fcm.googleapis.com" });
  expect(wrongAudience.status).toBe(403);
  const tooLong = await push(t, subscription, { expiresAt: t.clock.now + 25 * 3600 });
  expect(tooLong.status).toBe(403);
  const expired = await push(t, subscription, { expiresAt: t.clock.now - 3600 });
  expect(expired.status).toBe(403);
  expect(t.fcm.requests).toHaveLength(0);
});

test("requires Content-Encoding: aes128gcm and TTL, and validates Urgency and Topic", async () => {
  const { t, subscription } = await ready();
  const cases: [Record<string, string | null>, number, string][] = [
    [{ "content-encoding": null }, 415, "unsupported_content_encoding"],
    [{ "content-encoding": "aesgcm" }, 415, "unsupported_content_encoding"],
    [{ ttl: null }, 400, "ttl_required"],
    [{ ttl: "-1" }, 400, "ttl_required"],
    [{ ttl: "soon" }, 400, "ttl_required"],
    [{ urgency: "urgent" }, 400, "invalid_urgency"],
    [{ topic: "x".repeat(33) }, 400, "invalid_topic"],
    [{ topic: "has space" }, 400, "invalid_topic"],
  ];
  for (const [headers, status, code] of cases) {
    const response = await push(t, subscription, { headers });
    expect(response.status).toBe(status);
    expect(((await response.json()) as { error: string }).error).toBe(code);
  }
  expect(t.fcm.requests).toHaveLength(0);
});

test("accepts TTL 0 and every RFC 8030 urgency", async () => {
  const { t, subscription } = await ready();
  for (const urgency of ["very-low", "low", "normal", "high"]) {
    expect((await push(t, subscription, { headers: { ttl: "0", urgency } })).status).toBe(201);
  }
  expect(t.fcm.requests.map((request) => request.urgency)).toEqual(["very-low", "low", "normal", "high"]);
  expect(t.fcm.requests.every((request) => request.ttl === 0)).toBe(true);
});

test("answers 413 above the body limit, whether declared or streamed", async () => {
  const { t, subscription } = await ready();
  const atLimit = await encryptWebPush({
    uaPublic: subscription.userAgent.publicKey,
    authSecret: subscription.userAgent.authSecret,
    plaintext: new Uint8Array(MAX_PUSH_BODY_BYTES - 86 - 17),
  });
  expect(atLimit.length).toBe(MAX_PUSH_BODY_BYTES);
  expect((await push(t, subscription, { body: atLimit })).status).toBe(201);

  const over = await encryptWebPush({
    uaPublic: subscription.userAgent.publicKey,
    authSecret: subscription.userAgent.authSecret,
    plaintext: new Uint8Array(MAX_PUSH_BODY_BYTES - 86 - 16),
  });
  expect((await push(t, subscription, { body: over })).status).toBe(413);

  // No Content-Length: a streamed body is cut off as soon as it passes the limit.
  const init = await pushInit(t, subscription, { body: over });
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(over.slice(0, 1000));
      controller.enqueue(over.slice(1000));
      controller.close();
    },
  });
  const streamed = await t.relay.fetch(
    new Request(`${ORIGIN}${subscription.path}`, { method: "POST", headers: init.headers, body: stream, duplex: "half" } as RequestInit),
  );
  expect(streamed.status).toBe(413);
  expect(t.fcm.requests).toHaveLength(1);
});

test("rejects a body that is not a single-record aes128gcm Web Push message", async () => {
  const { t, subscription } = await ready();
  const valid = await encryptWebPush({
    uaPublic: subscription.userAgent.publicKey,
    authSecret: subscription.userAgent.authSecret,
    plaintext: utf8("hello"),
  });
  const shortKeyId = valid.slice();
  shortKeyId[20] = 32;
  const tinyRecord = valid.slice();
  new DataView(tinyRecord.buffer).setUint32(16, 18);
  for (const body of [new Uint8Array(0), new Uint8Array(50), shortKeyId, tinyRecord]) {
    const response = await push(t, subscription, { body });
    expect(response.status).toBe(400);
  }
  expect(t.fcm.requests).toHaveLength(0);
});

test("answers 410 once APNs/FCM reports the device invalid, until the token is replaced", async () => {
  const { t, device, subscription } = await ready("apns");
  t.apns.next({ kind: "device-invalid", reason: "Unregistered", evidence: "expired" });
  expect((await push(t, subscription)).status).toBe(410);
  expect((await push(t, subscription)).status).toBe(410);
  expect(t.apns.requests).toHaveLength(1);

  const key = base64UrlEncode((await generateVapidKeys()).publicKey);
  const blocked = await t.call("POST", `/v1/devices/${device.deviceId}/subscriptions`, {
    headers: device.auth,
    body: { application_server_key: key },
  });
  expect(blocked.status).toBe(409);

  const update = await t.call("PUT", `/v1/devices/${device.deviceId}`, {
    headers: device.auth,
    body: { token: "c3".repeat(32), apns_environment: "development" },
  });
  expect(update.status).toBe(204);
  expect((await push(t, subscription)).status).toBe(201);
  expect(t.apns.requests.at(-1)?.token).toBe("c3".repeat(32));
});

test("purges invalid devices after the retention period, after which endpoints answer 404", async () => {
  const { t, subscription } = await ready("fcm", { invalidRetentionSeconds: 3600 });
  t.fcm.next({ kind: "device-invalid", reason: "UNREGISTERED", evidence: "expired" });
  expect((await push(t, subscription)).status).toBe(410);
  t.clock.now += 3601;
  await t.relay.scheduled();
  expect((await push(t, subscription)).status).toBe(404);
});

test("limits pushes per endpoint and per device with 429 and Retry-After", async () => {
  const { t, device, subscription } = await ready("fcm", { endpointPerMinute: 2, devicePerMinute: 3 });
  t.clock.now = 1_800_000_010; // 50 seconds before the minute window ends
  expect((await push(t, subscription)).status).toBe(201);
  expect((await push(t, subscription)).status).toBe(201);
  const limited = await push(t, subscription);
  expect(limited.status).toBe(429);
  expect(limited.headers.get("retry-after")).toBe("50");

  // A second endpoint on the same device still has its own budget, until the
  // device budget (3) is spent.
  const second = await subscribe(t, device);
  expect((await push(t, second)).status).toBe(201);
  const deviceLimited = await push(t, second);
  expect(deviceLimited.status).toBe(429);
  expect(t.fcm.requests).toHaveLength(3);

  t.clock.now += 50;
  expect((await push(t, subscription)).status).toBe(201);
});

test("does not count requests that fail VAPID against the sender's budget", async () => {
  const { t, subscription } = await ready("fcm", { endpointPerMinute: 1 });
  const stranger = await generateVapidKeys();
  for (let index = 0; index < 5; index += 1) {
    expect((await push(t, subscription, { vapid: stranger })).status).toBe(403);
  }
  expect((await push(t, subscription)).status).toBe(201);
});

test("maps upstream throttling, outages and failures to retryable or failed answers", async () => {
  const { t, subscription } = await ready();
  t.fcm.next({ kind: "throttled", retryAfter: 120, reason: "QUOTA_EXCEEDED" });
  const throttled = await push(t, subscription);
  expect(throttled.status).toBe(429);
  expect(throttled.headers.get("retry-after")).toBe("120");

  t.fcm.next({ kind: "unavailable", retryAfter: null, reason: "UNAVAILABLE" });
  const unavailable = await push(t, subscription);
  expect(unavailable.status).toBe(503);
  expect(unavailable.headers.get("retry-after")).toBe("60");

  t.fcm.next({ kind: "failed", reason: "INVALID_ARGUMENT" });
  expect((await push(t, subscription)).status).toBe(502);
  // None of these invalidate the device.
  expect((await push(t, subscription)).status).toBe(201);
});

test("answers 503 when the device's platform has no sender configured", async () => {
  const t = createTestRelay({ senders: { apns: null } });
  const device = await register(t, { platform: "apns", token: APNS_TOKEN });
  const subscription = await subscribe(t, device);
  const response = await push(t, subscription);
  expect(response.status).toBe(503);
  expect(response.headers.get("retry-after")).toBe("3600");
});

test("only POST is allowed on a push resource, and messages are not retained", async () => {
  const { t, subscription } = await ready();
  const get = await t.call("GET", subscription.path);
  expect(get.status).toBe(405);
  expect(get.headers.get("allow")).toBe("POST");
  const created = await push(t, subscription);
  const location = new URL(created.headers.get("location") ?? "");
  expect((await t.call("DELETE", location.pathname)).status).toBe(404);
});

test("never logs the body, the VAPID JWT or sub, the token, or an app origin", async () => {
  const { t, subscription } = await ready("apns");
  const init = await pushInit(t, subscription);
  await t.call("POST", subscription.path, init);
  await push(t, subscription, { vapid: await generateVapidKeys() });
  t.apns.next({ kind: "device-invalid", reason: "BadDeviceToken", evidence: "rejected" });
  await push(t, subscription);

  const logged = t.log.text();
  const jwt = init.headers.authorization?.match(/t=([^,]+)/u)?.[1] ?? "";
  expect(jwt.length).toBeGreaterThan(20);
  for (const forbidden of [
    SECRET_PAYLOAD,
    base64UrlEncode(init.body),
    base64UrlEncode(init.body).slice(0, 40),
    jwt,
    jwt.split(".")[1] ?? "",
    VAPID_SUBJECT,
    "operator-who-must-not-be-logged",
    APNS_TOKEN,
    subscription.path,
  ]) {
    expect(logged).not.toContain(forbidden);
  }
  expect(logged).toContain("push_delivered");
  expect(base64UrlDecode(base64UrlEncode(init.body))?.length).toBe(init.body.length);
});

test("configuration errors from APNs/FCM answer 502 and never invalidate the device", async () => {
  for (const [platform, reason] of [
    ["apns", "DeviceTokenNotForTopic"],
    ["apns", "InvalidProviderToken"],
    ["apns", "TopicDisallowed"],
    ["fcm", "SENDER_ID_MISMATCH"],
    ["fcm", "PERMISSION_DENIED"],
  ] as const) {
    const { t, subscription } = await ready(platform);
    const sender = platform === "apns" ? t.apns : t.fcm;
    sender.always({ kind: "failed", reason });
    for (let index = 0; index < 3; index += 1) {
      expect((await push(t, subscription)).status).toBe(502);
    }
    expect(t.sqlite.query("SELECT invalidated_at FROM devices").get()).toEqual({ invalidated_at: null });
    expect(t.log.text()).toContain(`"reason":"${reason}"`);
    sender.always({ kind: "delivered" });
    expect((await push(t, subscription)).status).toBe(201);
  }
});

async function devicesOn(t: ReturnType<typeof createTestRelay>, count: number) {
  const subscriptions = [];
  for (let index = 0; index < count; index += 1) {
    subscriptions.push(await subscribe(t, await register(t)));
  }
  return subscriptions;
}

/** Devices that each received one push already, so the breaker guards them. */
async function deliveredDevices(t: ReturnType<typeof createTestRelay>, count: number) {
  const subscriptions = await devicesOn(t, count);
  for (const subscription of subscriptions) expect((await push(t, subscription)).status).toBe(201);
  return subscriptions;
}

const REJECTED = { kind: "device-invalid", reason: "BadDeviceToken", evidence: "rejected" } as const;
const EXPIRED = { kind: "device-invalid", reason: "UNREGISTERED", evidence: "expired" } as const;

test("the invalidation breaker stops mass rejection of working devices after an absolute count and alerts", async () => {
  const t = createTestRelay({ limits: { breakerMaxInvalidations: 2, breakerMinDeliveries: 1000 } });
  const subscriptions = await deliveredDevices(t, 4);
  t.fcm.always(REJECTED);
  const statuses = [];
  for (const subscription of subscriptions) statuses.push((await push(t, subscription)).status);
  expect(statuses).toEqual([410, 410, 502, 502]);
  expect(t.sqlite.query("SELECT COUNT(*) AS n FROM devices WHERE invalidated_at IS NULL").get()).toEqual({ n: 2 });
  const alert = t.log.lines.map((line) => JSON.parse(line)).find((line) => line.event === "invalidation_breaker_open");
  expect(alert).toMatchObject({ level: "error", alert: true, platform: "fcm", invalidations: 3 });

  // The window passes and the breaker closes.
  t.clock.now += 600;
  expect((await push(t, subscriptions[2] as (typeof subscriptions)[number])).status).toBe(410);
});

test("the invalidation breaker opens when rejections exceed a share of deliveries", async () => {
  const t = createTestRelay({ limits: { breakerMaxInvalidations: 100, breakerMinDeliveries: 6, breakerMaxRatio: 0.25 } });
  const [a, b, c] = (await deliveredDevices(t, 3)) as [Subscribed, Subscribed, Subscribed];
  t.fcm.always(REJECTED);
  // 3 deliveries before, then a's rejection: 1 of 4, below the minimum count, trusted.
  expect((await push(t, a)).status).toBe(410);
  // 1 delivery more each: b is the 5th delivery and 2nd rejection, still under 6.
  expect((await push(t, b)).status).toBe(410);
  // c: 6 deliveries, 3 rejections, half of them: the breaker opens.
  expect((await push(t, c)).status).toBe(502);
});

test("expired tokens always invalidate, even while the breaker is open", async () => {
  const t = createTestRelay({ limits: { breakerMaxInvalidations: 1, breakerMinDeliveries: 1000 } });
  const working = await deliveredDevices(t, 3);
  t.fcm.always(REJECTED);
  expect((await push(t, working[0] as Subscribed)).status).toBe(410);
  expect((await push(t, working[1] as Subscribed)).status).toBe(502); // breaker open
  t.fcm.always(EXPIRED);
  expect((await push(t, working[2] as Subscribed)).status).toBe(410);
  expect(t.log.text()).toContain('"evidence":"expired"');
});

test("junk tokens that never worked cannot hold the breaker open", async () => {
  const t = createTestRelay({ limits: { breakerMaxInvalidations: 2, breakerMinDeliveries: 1000 } });
  // An attacker registers many tokens that APNs/FCM reject on the first push.
  const junk = await devicesOn(t, 25);
  t.fcm.always(REJECTED);
  for (const subscription of junk) expect((await push(t, subscription)).status).toBe(410);
  expect(t.log.text()).not.toContain("invalidation_breaker_open");

  // A real device that was uninstalled is still invalidated.
  const [real] = (await deliveredDevicesWith(t, 1)) as [Subscribed];
  t.fcm.always(EXPIRED);
  expect((await push(t, real)).status).toBe(410);
});

async function deliveredDevicesWith(t: ReturnType<typeof createTestRelay>, count: number) {
  t.fcm.always({ kind: "delivered" });
  return deliveredDevices(t, count);
}

test("a token replaced while a push is in flight is not invalidated by the old token's rejection", async () => {
  const { t, device, subscription } = await ready("apns");
  const fresh = "d4".repeat(32);
  t.apns.next({ kind: "device-invalid", reason: "Unregistered", evidence: "expired" });
  t.apns.during = async () => {
    t.apns.during = null;
    const update = await t.call("PUT", `/v1/devices/${device.deviceId}`, {
      headers: device.auth,
      body: { token: fresh, apns_environment: "development" },
    });
    expect(update.status).toBe(204);
  };
  const raced = await push(t, subscription);
  expect(raced.status).toBe(503);
  expect(raced.headers.get("retry-after")).toBe("5");
  expect(t.sqlite.query("SELECT token, invalidated_at FROM devices").get()).toEqual({ token: fresh, invalidated_at: null });
  expect((await push(t, subscription)).status).toBe(201);
  expect(t.apns.requests.at(-1)?.token).toBe(fresh);
});

test("busy endpoints cannot starve another app on the same device (fair share)", async () => {
  const { t, device, subscription: busy } = await ready("fcm", { endpointPerMinute: 5, devicePerMinute: 6 });
  const quiet = await subscribe(t, device);
  for (let index = 0; index < 5; index += 1) expect((await push(t, busy)).status).toBe(201);
  expect((await push(t, busy)).status).toBe(429); // its own endpoint limit
  // One more from the quiet app reaches the device ceiling (6)...
  expect((await push(t, quiet)).status).toBe(201);
  // ...past which the quiet app may still send up to its fair share, 6 / 2 = 3.
  expect((await push(t, quiet)).status).toBe(201);
  expect((await push(t, quiet)).status).toBe(201);
  expect((await push(t, quiet)).status).toBe(429);

  // A third app arriving now still gets its share, 6 / 3 = 2.
  const late = await subscribe(t, device);
  expect((await push(t, late)).status).toBe(201);
  expect((await push(t, late)).status).toBe(201);
  expect((await push(t, late)).status).toBe(429);
});
