import { expect, test } from "bun:test";
import { base64UrlEncode, sha256Hex } from "../src/encoding.ts";
import { generateVapidKeys } from "../tools/web-push.ts";
import { APNS_TOKEN, FCM_TOKEN, ORIGIN, createTestRelay, push, register, subscribe } from "./support/relay.ts";

test("registers an APNs device and stores only the token and a secret hash", async () => {
  const t = createTestRelay();
  const device = await register(t, { platform: "apns", token: APNS_TOKEN.toUpperCase(), apns_environment: "development" });
  expect(device.deviceId).toMatch(/^[A-Za-z0-9_-]{22}$/u);
  expect(device.secret.length).toBeGreaterThanOrEqual(43);
  const row = t.sqlite.query("SELECT * FROM devices").get() as Record<string, unknown>;
  expect(row).toMatchObject({
    device_id: device.deviceId,
    platform: "apns",
    token: APNS_TOKEN,
    apns_environment: "development",
    invalidated_at: null,
  });
  expect(row.secret_hash).toBe(await sha256Hex(device.secret));
  expect(JSON.stringify(row)).not.toContain(device.secret);
});

test("defaults an APNs device to the production environment and an FCM device to none", async () => {
  const t = createTestRelay();
  await register(t, { platform: "apns", token: APNS_TOKEN });
  await register(t, { platform: "fcm", token: FCM_TOKEN });
  const rows = t.sqlite.query("SELECT platform, apns_environment FROM devices ORDER BY platform").all();
  expect(rows).toEqual([
    { platform: "apns", apns_environment: "production" },
    { platform: "fcm", apns_environment: null },
  ]);
});

test("rejects malformed registrations", async () => {
  const t = createTestRelay();
  const cases: [unknown, string][] = [
    [{ platform: "webpush", token: FCM_TOKEN }, "invalid_platform"],
    [{ platform: "apns", token: "not-hex" }, "invalid_token"],
    [{ platform: "apns", token: APNS_TOKEN, apns_environment: "sandbox" }, "invalid_apns_environment"],
    [{ platform: "fcm", token: "short" }, "invalid_token"],
    [{ platform: "fcm", token: FCM_TOKEN, apns_environment: "production" }, "invalid_apns_environment"],
    [["not", "an", "object"], "invalid_json"],
  ];
  for (const [body, code] of cases) {
    const response = await t.call("POST", "/v1/devices", { body });
    expect(response.status).toBe(400);
    expect(((await response.json()) as { error: string }).error).toBe(code);
  }
  expect(t.sqlite.query("SELECT COUNT(*) AS n FROM devices").get()).toEqual({ n: 0 });
});

test("issues an endpoint under the relay origin with a 256-bit random id", async () => {
  const t = createTestRelay();
  const device = await register(t);
  const subscription = await subscribe(t, device);
  expect(subscription.endpoint.startsWith(`${ORIGIN}/push/`)).toBe(true);
  expect(subscription.endpoint.slice(`${ORIGIN}/push/`.length)).toMatch(/^[A-Za-z0-9_-]{43}$/u);
  const row = t.sqlite.query("SELECT * FROM subscriptions").get() as Record<string, unknown>;
  expect(Object.keys(row).sort()).toEqual(
    ["application_server_key", "created_at", "device_id", "endpoint_id", "subscription_id"].sort(),
  );
  expect(row.application_server_key).toBe(base64UrlEncode(subscription.vapid.publicKey));
});

test("uses the request origin when no relay origin is configured", async () => {
  const t = createTestRelay({ origin: null });
  const device = await register(t);
  const subscription = await subscribe(t, device);
  expect(subscription.endpoint.startsWith(`${ORIGIN}/push/`)).toBe(true);
});

test("rejects an application server key that is not a P-256 point", async () => {
  const t = createTestRelay();
  const device = await register(t);
  const bogus = new Uint8Array(65);
  bogus[0] = 4;
  for (const key of [base64UrlEncode(bogus), "short", 42]) {
    const response = await t.call("POST", `/v1/devices/${device.deviceId}/subscriptions`, {
      headers: device.auth,
      body: { application_server_key: key },
    });
    expect(response.status).toBe(400);
  }
});

test("requires the device secret for everything after registration", async () => {
  const t = createTestRelay();
  const device = await register(t);
  const other = await register(t);
  const key = base64UrlEncode((await generateVapidKeys()).publicKey);
  const attempts: [string, string, unknown][] = [
    ["PUT", `/v1/devices/${device.deviceId}`, { token: FCM_TOKEN }],
    ["DELETE", `/v1/devices/${device.deviceId}`, undefined],
    ["POST", `/v1/devices/${device.deviceId}/subscriptions`, { application_server_key: key }],
  ];
  const wrong: Record<string, string>[] = [
    {},
    { authorization: `Bearer ${other.secret}` },
    { authorization: device.secret },
  ];
  for (const headers of wrong) {
    for (const [method, path, body] of attempts) {
      const response = await t.call(method, path, { headers, body });
      expect(response.status).toBe(401);
      expect(response.headers.get("www-authenticate")).toBe("Bearer");
    }
  }
  const unknown = await t.call("DELETE", "/v1/devices/AAAAAAAAAAAAAAAAAAAAAA", { headers: device.auth });
  expect(unknown.status).toBe(401);
});

test("a token change keeps every endpoint", async () => {
  const t = createTestRelay();
  const device = await register(t);
  const subscription = await subscribe(t, device);
  const newToken = `${FCM_TOKEN}-rotated`;
  const response = await t.call("PUT", `/v1/devices/${device.deviceId}`, { headers: device.auth, body: { token: newToken } });
  expect(response.status).toBe(204);
  expect((await push(t, subscription)).status).toBe(201);
  expect(t.fcm.requests.at(-1)?.token).toBe(newToken);
  expect(t.sqlite.query("SELECT endpoint_id FROM subscriptions").get()).toEqual({
    endpoint_id: subscription.path.slice("/push/".length),
  });
});

test("a device cannot switch platform", async () => {
  const t = createTestRelay();
  const device = await register(t);
  const response = await t.call("PUT", `/v1/devices/${device.deviceId}`, {
    headers: device.auth,
    body: { platform: "apns", token: APNS_TOKEN },
  });
  expect(response.status).toBe(400);
});

test("deleting a subscription or the device makes its endpoints unknown", async () => {
  const t = createTestRelay();
  const device = await register(t);
  const first = await subscribe(t, device);
  const second = await subscribe(t, device);

  const removed = await t.call("DELETE", `/v1/devices/${device.deviceId}/subscriptions/${first.subscriptionId}`, {
    headers: device.auth,
  });
  expect(removed.status).toBe(204);
  expect((await push(t, first)).status).toBe(404);
  expect((await push(t, second)).status).toBe(201);
  const again = await t.call("DELETE", `/v1/devices/${device.deviceId}/subscriptions/${first.subscriptionId}`, {
    headers: device.auth,
  });
  expect(again.status).toBe(404);

  expect((await t.call("DELETE", `/v1/devices/${device.deviceId}`, { headers: device.auth })).status).toBe(204);
  expect((await push(t, second)).status).toBe(404);
  expect(t.sqlite.query("SELECT COUNT(*) AS n FROM subscriptions").get()).toEqual({ n: 0 });
  expect(t.sqlite.query("SELECT COUNT(*) AS n FROM devices").get()).toEqual({ n: 0 });
});

test("another device cannot delete someone else's subscription", async () => {
  const t = createTestRelay();
  const owner = await register(t);
  const intruder = await register(t);
  const subscription = await subscribe(t, owner);
  const response = await t.call("DELETE", `/v1/devices/${intruder.deviceId}/subscriptions/${subscription.subscriptionId}`, {
    headers: intruder.auth,
  });
  expect(response.status).toBe(404);
  expect((await push(t, subscription)).status).toBe(201);
});

test("limits subscriptions per device and registrations per address", async () => {
  const t = createTestRelay({ limits: { subscriptionsPerDevice: 2, registrationsPerHour: 3 } });
  const device = await register(t);
  await subscribe(t, device);
  await subscribe(t, device);
  const key = base64UrlEncode((await generateVapidKeys()).publicKey);
  const full = await t.call("POST", `/v1/devices/${device.deviceId}/subscriptions`, {
    headers: device.auth,
    body: { application_server_key: key },
  });
  expect(full.status).toBe(409);

  const address = { "cf-connecting-ip": "198.51.100.7" };
  for (let index = 0; index < 3; index += 1) {
    expect((await t.call("POST", "/v1/devices", { headers: address, body: { platform: "fcm", token: FCM_TOKEN } })).status).toBe(201);
  }
  const limited = await t.call("POST", "/v1/devices", { headers: address, body: { platform: "fcm", token: FCM_TOKEN } });
  expect(limited.status).toBe(429);
  expect(Number(limited.headers.get("retry-after"))).toBeGreaterThan(0);
  const elsewhere = await t.call("POST", "/v1/devices", {
    headers: { "cf-connecting-ip": "203.0.113.9" },
    body: { platform: "fcm", token: FCM_TOKEN },
  });
  expect(elsewhere.status).toBe(201);
  // The address is kept only as a hash inside an expiring counter.
  expect(JSON.stringify(t.sqlite.query("SELECT * FROM rate_windows").all())).not.toContain("198.51.100.7");
});

test("never stores or logs device secrets or tokens", async () => {
  const t = createTestRelay();
  const device = await register(t, { platform: "apns", token: APNS_TOKEN });
  await subscribe(t, device);
  await t.call("PUT", `/v1/devices/${device.deviceId}`, { headers: device.auth, body: { token: "b2".repeat(32) } });
  const logged = t.log.text();
  expect(logged).not.toContain(device.secret);
  expect(logged).not.toContain(APNS_TOKEN);
  expect(logged).not.toContain("b2".repeat(32));
});
