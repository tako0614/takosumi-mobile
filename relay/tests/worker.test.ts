import { expect, test } from "bun:test";
import { join } from "node:path";
import worker, { resolveConfig, type RelayEnv } from "../src/index.ts";
import { ApnsSender } from "../src/senders/apns.ts";
import { FcmSender } from "../src/senders/fcm.ts";
import { SinkSender } from "../src/senders/sink.ts";
import { generateUserAgentKeys, generateVapidKeys, sendWebPush } from "../tools/web-push.ts";
import { base64UrlEncode } from "../src/encoding.ts";
import { createSqliteD1 } from "./support/sqlite-d1.ts";

const ORIGIN = "https://push.relay.test";

function env(overrides: Partial<RelayEnv> = {}): RelayEnv {
  return { DB: createSqliteD1().db, RELAY_ENVIRONMENT: "staging", RELAY_ORIGIN: ORIGIN, ...overrides };
}

const call = (e: RelayEnv) => (input: string, init?: RequestInit) => worker.fetch(new Request(input, init), e);

test("sink delivery is allowed only in staging", () => {
  expect(resolveConfig(env({ DELIVERY_MODE: "sink" })).senders.apns).toBeInstanceOf(SinkSender);
  expect(resolveConfig(env({ DELIVERY_MODE: "sink", RELAY_ENVIRONMENT: "production" })).problems).toContain(
    "DELIVERY_MODE=sink is allowed only when RELAY_ENVIRONMENT=staging",
  );
  expect(resolveConfig(env({ DELIVERY_MODE: "sink", RELAY_ENVIRONMENT: undefined })).problems.length).toBe(1);
});

test("live delivery configures each platform only when all of its settings are present", () => {
  const none = resolveConfig(env());
  expect(none.senders).toEqual({ apns: null, fcm: null });
  expect(none.problems).toEqual([]);
  const both = resolveConfig(
    env({
      APNS_TEAM_ID: "T",
      APNS_KEY_ID: "K",
      APNS_TOPIC: "com.takosumi",
      APNS_PRIVATE_KEY: "pem",
      FCM_PROJECT_ID: "p",
      FCM_SERVICE_ACCOUNT_JSON: JSON.stringify({ client_email: "e", private_key: "k" }),
    }),
  );
  expect(both.senders.apns).toBeInstanceOf(ApnsSender);
  expect(both.senders.fcm).toBeInstanceOf(FcmSender);
  expect(resolveConfig(env({ FCM_PROJECT_ID: "p", FCM_SERVICE_ACCOUNT_JSON: "{" })).problems).toContain(
    "FCM_SERVICE_ACCOUNT_JSON is not a service account key",
  );
});

test("RELAY_ORIGIN must be a bare https origin", () => {
  expect(resolveConfig(env({ RELAY_ORIGIN: "http://push.example" })).problems.length).toBe(1);
  expect(resolveConfig(env({ RELAY_ORIGIN: "https://push.example/v1" })).problems.length).toBe(1);
  expect(resolveConfig(env({ RELAY_ORIGIN: "https://push.example/" })).origin).toBe("https://push.example");
  expect(resolveConfig(env({ RELAY_ORIGIN: undefined })).origin).toBeNull();
});

test("a misconfigured Worker refuses every request with 503", async () => {
  const response = await call(env({ DELIVERY_MODE: "sink", RELAY_ENVIRONMENT: "production" }))(`${ORIGIN}/healthz`);
  expect(response.status).toBe(503);
});

test("the Worker serves the full device → subscription → Web Push flow", async () => {
  const e = env({ DELIVERY_MODE: "sink" });
  const fetcher = call(e);
  expect(await (await fetcher(`${ORIGIN}/healthz`)).json()).toEqual({ service: "takosumi-push-relay", status: "ok" });

  const registered = await fetcher(`${ORIGIN}/v1/devices`, {
    method: "POST",
    body: JSON.stringify({ platform: "fcm", token: "smoke-token-0123456789abcdef" }),
  });
  expect(registered.status).toBe(201);
  const device = (await registered.json()) as { device_id: string; device_secret: string };
  const vapid = await generateVapidKeys();
  const subscribed = await fetcher(`${ORIGIN}/v1/devices/${device.device_id}/subscriptions`, {
    method: "POST",
    headers: { authorization: `Bearer ${device.device_secret}` },
    body: JSON.stringify({ application_server_key: base64UrlEncode(vapid.publicKey) }),
  });
  const { endpoint } = (await subscribed.json()) as { endpoint: string };

  const response = await sendWebPush({
    endpoint,
    vapid,
    userAgent: await generateUserAgentKeys(),
    payload: new TextEncoder().encode('{"web_push":8030,"notification":{"title":"t","navigate":"https://a.example/"}}'),
    ttl: 60,
    fetcher,
  });
  expect(response.status).toBe(201);

  await worker.scheduled(undefined, e);
});

test("the Worker bundles for the Workers runtime without Node built-ins", async () => {
  const result = await Bun.build({
    entrypoints: [join(import.meta.dir, "..", "src", "index.ts")],
    target: "browser",
    format: "esm",
  });
  expect(result.success).toBe(true);
  const source = await result.outputs[0]?.text();
  expect(source).toBeDefined();
  expect(source).not.toMatch(/from\s*["']node:|require\(["']node:/u);
  expect(source).toContain("export");
});
