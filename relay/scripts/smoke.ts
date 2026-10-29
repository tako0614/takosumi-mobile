#!/usr/bin/env bun

// Post-deploy smoke for the push relay, run as an application server and a
// device would use it:
//
//   bun run relay:smoke -- --origin https://<relay> --expect sink
//   bun run relay:smoke -- --origin https://<relay> --expect live --platform apns --platform fcm
//
// With `--expect sink` (staging without APNs/FCM) an encrypted Web Push must be
// answered 201. With `--expect live` the device token is fake, so APNs/FCM must
// reject that token and the relay must answer 410: that proves the relay reached
// the platform with credentials it accepted. A configuration or credential
// error (wrong key, team, topic or project) comes back as 502 and fails the
// smoke. A fake token cannot prove the APNs topic or Firebase project binding
// itself; a real device's first push does. Every device the smoke registers is
// deleted again. Prints one JSON summary; exits non-zero on any mismatch.

import { base64UrlEncode, randomId } from "../src/encoding.ts";
import { generateUserAgentKeys, generateVapidKeys, sendWebPush, vapidAuthorization } from "../tools/web-push.ts";

interface Check {
  name: string;
  expected: number | string;
  actual: number | string;
  ok: boolean;
}

function parseArgs(argv: string[]) {
  let origin: string | null = null;
  let expectation: "sink" | "live" = "sink";
  const platforms: ("apns" | "fcm")[] = [];
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const value = argv[index + 1];
    if (arg === "--origin" && value) {
      origin = value.replace(/\/$/u, "");
      index += 1;
    } else if (arg === "--expect" && (value === "sink" || value === "live")) {
      expectation = value;
      index += 1;
    } else if (arg === "--platform" && (value === "apns" || value === "fcm")) {
      platforms.push(value);
      index += 1;
    } else {
      throw new Error(`unknown argument ${arg}`);
    }
  }
  // http is accepted only for a relay on this machine (the tests run one).
  if (!origin || !/^(?:https:\/\/|http:\/\/(?:127\.0\.0\.1|localhost)(?::\d+)?$)/u.test(origin)) {
    throw new Error("--origin https://<relay> is required");
  }
  return { origin, expectation, platforms: platforms.length > 0 ? platforms : (["fcm"] as ("apns" | "fcm")[]) };
}

const checks: Check[] = [];
function check(name: string, expected: number | string, actual: number | string) {
  checks.push({ name, expected, actual, ok: expected === actual });
}

async function main() {
  const { origin, expectation, platforms } = parseArgs(process.argv.slice(2));
  const health = await fetch(`${origin}/healthz`);
  const healthBody = (await health.json().catch(() => ({}))) as { service?: string };
  check("health", 200, health.status);
  check("service", "takosumi-push-relay", healthBody.service ?? "");

  if (expectation === "live" && platforms.length === 0) throw new Error("live smoke needs at least one --platform");
  for (const platform of platforms) {
    const token = platform === "apns" ? "ab".repeat(32) : `relay-smoke-${randomId(24)}`;
    const registered = await fetch(`${origin}/v1/devices`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(
        platform === "apns" ? { platform, token, apns_environment: "development" } : { platform, token },
      ),
    });
    check(`${platform}: register`, 201, registered.status);
    if (registered.status !== 201) continue;
    const device = (await registered.json()) as { device_id: string; device_secret: string };
    const auth = { authorization: `Bearer ${device.device_secret}`, "content-type": "application/json" };
    try {
      const vapid = await generateVapidKeys();
      const subscribed = await fetch(`${origin}/v1/devices/${device.device_id}/subscriptions`, {
        method: "POST",
        headers: auth,
        body: JSON.stringify({ application_server_key: base64UrlEncode(vapid.publicKey) }),
      });
      check(`${platform}: subscribe`, 201, subscribed.status);
      if (subscribed.status !== 201) continue;
      const { endpoint } = (await subscribed.json()) as { endpoint: string };
      check(`${platform}: endpoint origin`, new URL(origin).origin, new URL(endpoint).origin);
      const userAgent = await generateUserAgentKeys();
      const payload = new TextEncoder().encode(
        JSON.stringify({ web_push: 8030, notification: { title: "Relay smoke", navigate: "https://relay-smoke.invalid/" } }),
      );

      const unsigned = await fetch(endpoint, {
        method: "POST",
        headers: { "content-encoding": "aes128gcm", ttl: "60" },
        body: new Uint8Array(120),
      });
      check(`${platform}: push without VAPID`, 401, unsigned.status);
      const stranger = await generateVapidKeys();
      const forged = await fetch(endpoint, {
        method: "POST",
        headers: {
          authorization: await vapidAuthorization(stranger, new URL(endpoint).origin, { expiresAt: Math.floor(Date.now() / 1000) + 3600 }),
          "content-encoding": "aes128gcm",
          ttl: "60",
        },
        body: new Uint8Array(120),
      });
      check(`${platform}: push with another key`, 403, forged.status);
      const unknown = await sendWebPush({ endpoint: `${origin}/push/${randomId(32)}`, vapid, userAgent, payload, ttl: 60 });
      check(`${platform}: unknown endpoint`, 404, unknown.status);

      const sent = await sendWebPush({ endpoint, vapid, userAgent, payload, ttl: 60, urgency: "high", topic: "smoke" });
      check(`${platform}: Web Push`, expectation === "sink" ? 201 : 410, sent.status);
      if (sent.status === 502) {
        const detail = (await sent.json().catch(() => ({}))) as { error?: string };
        check(`${platform}: relay configuration (502 means APNs/FCM refused our key, team, topic or project)`, "accepted", `refused:${detail.error ?? "unknown"}`);
      }
      if (sent.status === 201) {
        check(`${platform}: Location`, "present", sent.headers.get("location") ? "present" : "missing");
      }
    } finally {
      const removed = await fetch(`${origin}/v1/devices/${device.device_id}`, { method: "DELETE", headers: auth });
      check(`${platform}: delete device`, 204, removed.status);
    }
  }

  const ok = checks.every((entry) => entry.ok);
  process.stdout.write(`${JSON.stringify({ kind: "takosumi.push-relay-smoke@v1", origin, expectation, ok, checks }, null, 2)}\n`);
  if (!ok) process.exit(1);
}

main().catch((error) => {
  process.stderr.write(`relay smoke failed: ${error instanceof Error ? error.message : String(error)}\n`);
  process.exit(1);
});
