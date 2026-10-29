import { expect, test } from "bun:test";
import { base64UrlEncode, sha256Hex } from "../src/encoding.ts";
import { generateVapidKeys } from "../tools/web-push.ts";
import { ipv6Prefix64 } from "../src/devices.ts";
import { APNS_TOKEN, FCM_TOKEN, ORIGIN, createTestRelay, push, register, subscribe, uniqueFcmToken } from "./support/relay.ts";

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
    [{ platform: "apns", token: "a1".repeat(31) }, "invalid_token"],
    [{ platform: "apns", token: "a1".repeat(33) }, "invalid_token"],
    [{ platform: "fcm", token: "short" }, "invalid_token"],
    [{ platform: "fcm", token: "f".repeat(1025) }, "invalid_token"],
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

test("registering a token another device holds supersedes that device", async () => {
  const t = createTestRelay();
  const token = uniqueFcmToken();
  const first = await register(t, { platform: "fcm", token });
  const oldSubscription = await subscribe(t, first);
  const second = await register(t, { platform: "fcm", token });
  const newSubscription = await subscribe(t, second);

  expect((await push(t, oldSubscription)).status).toBe(410);
  expect((await push(t, newSubscription)).status).toBe(201);
  expect(t.sqlite.query("SELECT device_id FROM devices WHERE token = ?1 AND invalidated_at IS NULL").all(token)).toEqual([
    { device_id: second.deviceId },
  ]);
  // The superseded device must send a new token before it can subscribe again.
  const key = base64UrlEncode((await generateVapidKeys()).publicKey);
  const blocked = await t.call("POST", `/v1/devices/${first.deviceId}/subscriptions`, {
    headers: first.auth,
    body: { application_server_key: key },
  });
  expect(blocked.status).toBe(409);
});

test("moving a device to a token another device holds supersedes that device", async () => {
  const t = createTestRelay();
  const holder = await register(t);
  const holderSubscription = await subscribe(t, holder);
  const mover = await register(t);
  const moverSubscription = await subscribe(t, mover);
  const holderToken = (t.sqlite.query("SELECT token FROM devices WHERE device_id = ?1").get(holder.deviceId) as { token: string }).token;
  expect((await t.call("PUT", `/v1/devices/${mover.deviceId}`, { headers: mover.auth, body: { token: holderToken } })).status).toBe(204);
  expect((await push(t, holderSubscription)).status).toBe(410);
  expect((await push(t, moverSubscription)).status).toBe(201);
  expect(t.fcm.requests.at(-1)?.token).toBe(holderToken);
});

test("the database refuses two active devices with one token", async () => {
  const t = createTestRelay();
  const device = await register(t, { platform: "fcm", token: FCM_TOKEN });
  expect(() =>
    t.sqlite
      .query("INSERT INTO devices (device_id, secret_hash, platform, token, created_at, updated_at, seen_at) VALUES ('x', 'h', 'fcm', ?1, 0, 0, 0)")
      .run(FCM_TOKEN),
  ).toThrow();
  expect(device.deviceId.length).toBeGreaterThan(0);
});

test("caps how many device rows one token may accumulate", async () => {
  const t = createTestRelay({ limits: { devicesPerToken: 3 } });
  const token = uniqueFcmToken();
  for (let index = 0; index < 3; index += 1) await register(t, { platform: "fcm", token });
  const capped = await t.call("POST", "/v1/devices", { body: { platform: "fcm", token } });
  expect(capped.status).toBe(429);
  expect(((await capped.json()) as { error: string }).error).toBe("token_registration_limit");
});

test("counts registrations per IPv6 /64 and per IPv4 address", async () => {
  expect(ipv6Prefix64("2001:db8:1:2::1")).toBe("2001:db8:1:2");
  expect(ipv6Prefix64("2001:0db8:0001:0002:ffff:0:0:5")).toBe("2001:db8:1:2");
  expect(ipv6Prefix64("::1")).toBe("0:0:0:0");
  expect(ipv6Prefix64("::ffff:192.0.2.1")).toBe("0:0:0:0");
  expect(ipv6Prefix64("192.0.2.1")).toBeNull();
  expect(ipv6Prefix64("1:2:3")).toBeNull();

  const t = createTestRelay({ limits: { registrationsPerHour: 2 } });
  const from = (address: string) => ({ "cf-connecting-ip": address });
  await register(t, undefined, from("2001:db8:1:2::1"));
  await register(t, undefined, from("2001:db8:1:2:aaaa:bbbb:cccc:dddd"));
  const sameNetwork = await t.call("POST", "/v1/devices", {
    headers: from("2001:db8:1:2:ffff::5"),
    body: { platform: "fcm", token: uniqueFcmToken() },
  });
  expect(sameNetwork.status).toBe(429);
  await register(t, undefined, from("2001:db8:1:3::1"));
  await register(t, undefined, from("192.0.2.1"));
  await register(t, undefined, from("192.0.2.2"));
});

test("a global registration limit holds over all clients", async () => {
  const t = createTestRelay({ limits: { registrationsPerHourGlobal: 3 } });
  for (let index = 1; index <= 3; index += 1) await register(t, undefined, { "cf-connecting-ip": `198.51.100.${index}` });
  const limited = await t.call("POST", "/v1/devices", {
    headers: { "cf-connecting-ip": "198.51.100.200" },
    body: { platform: "fcm", token: uniqueFcmToken() },
  });
  expect(limited.status).toBe(429);
  expect(t.log.text()).toContain("registration_global_limit");
});

test("devices unseen for the inactivity period are deleted; any authenticated call keeps one alive", async () => {
  const t = createTestRelay({ limits: { inactiveDeviceSeconds: 1000 } });
  const idle = await register(t);
  const idleSubscription = await subscribe(t, idle);
  const active = await register(t);
  const activeSubscription = await subscribe(t, active);

  t.clock.now += 900;
  const token = (t.sqlite.query("SELECT token FROM devices WHERE device_id = ?1").get(active.deviceId) as { token: string }).token;
  expect((await t.call("PUT", `/v1/devices/${active.deviceId}`, { headers: active.auth, body: { token } })).status).toBe(204);
  t.clock.now += 200;
  await t.relay.scheduled();

  expect((await push(t, idleSubscription)).status).toBe(404);
  expect((await push(t, activeSubscription)).status).toBe(201);
  expect(t.sqlite.query("SELECT device_id FROM devices").all()).toEqual([{ device_id: active.deviceId }]);
});

test("a device holds at most 64 subscriptions by default", async () => {
  const { DEFAULT_LIMITS } = await import("../src/context.ts");
  expect(DEFAULT_LIMITS.subscriptionsPerDevice).toBe(64);
});
