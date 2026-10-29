import { afterEach, expect, test } from "bun:test";
import { join } from "node:path";
import { createRelay } from "../src/app.ts";
import { DEFAULT_LIMITS } from "../src/context.ts";
import { D1Counters } from "../src/rate-limit.ts";
import { RelayStore } from "../src/store.ts";
import type { DeliveryOutcome } from "../src/senders/types.ts";
import { CapturingLog, MockSender } from "./support/relay.ts";
import { createSqliteD1 } from "./support/sqlite-d1.ts";

// The post-deploy smoke, run against a relay on this machine whose APNs/FCM
// senders are mocks: it must pass when APNs/FCM reject the fake token, and fail
// when they refuse the relay's own configuration.

let server: ReturnType<typeof Bun.serve> | null = null;
afterEach(() => {
  server?.stop(true);
  server = null;
});

function serve(outcome: DeliveryOutcome) {
  const { db } = createSqliteD1();
  const sender = new MockSender().always(outcome);
  const relay = createRelay({
    store: new RelayStore(db),
    counters: new D1Counters(db),
    senders: { apns: sender, fcm: sender },
    origin: null,
    limits: DEFAULT_LIMITS,
    log: new CapturingLog(),
    now: () => Math.floor(Date.now() / 1000),
  });
  server = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: (request) => relay.fetch(request) });
  return `http://127.0.0.1:${server.port}`;
}

async function smoke(origin: string, ...args: string[]) {
  const process = Bun.spawn(["bun", join(import.meta.dir, "..", "scripts", "smoke.ts"), "--origin", origin, ...args], {
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, code] = await Promise.all([new Response(process.stdout).text(), process.exited]);
  return { code, report: JSON.parse(stdout) as { ok: boolean; checks: { name: string; actual: unknown; ok: boolean }[] } };
}

test("live smoke passes when APNs/FCM reject the fake token (410)", async () => {
  const origin = serve({ kind: "device-invalid", reason: "BadDeviceToken" });
  const result = await smoke(origin, "--expect", "live", "--platform", "apns", "--platform", "fcm");
  expect(result.code).toBe(0);
  expect(result.report.ok).toBe(true);
});

test("live smoke fails on a configuration error: 502, not 410", async () => {
  for (const reason of ["DeviceTokenNotForTopic", "InvalidProviderToken", "SENDER_ID_MISMATCH"]) {
    const origin = serve({ kind: "failed", reason });
    const result = await smoke(origin, "--expect", "live", "--platform", "apns");
    expect(result.code).toBe(1);
    const push = result.report.checks.find((check) => check.name === "apns: Web Push");
    expect(push).toMatchObject({ actual: 502, ok: false });
    expect(result.report.checks.some((check) => check.name.startsWith("apns: relay configuration") && !check.ok)).toBe(true);
    // The smoke cleaned up after itself.
    expect(result.report.checks.find((check) => check.name === "apns: delete device")).toMatchObject({ actual: 204 });
    server?.stop(true);
  }
});

test("sink smoke passes against a relay that accepts every message", async () => {
  const origin = serve({ kind: "delivered" });
  const result = await smoke(origin, "--expect", "sink");
  expect(result.code).toBe(0);
});
