import type { Database } from "bun:sqlite";
import { createRelay } from "../../src/app.ts";
import { DEFAULT_LIMITS, type RelayLimits } from "../../src/context.ts";
import type { LogFields, RelayLog } from "../../src/log.ts";
import { D1Counters } from "../../src/rate-limit.ts";
import type { DeliveryOutcome, DeliveryRequest, PushSender } from "../../src/senders/types.ts";
import { RelayStore, type Platform } from "../../src/store.ts";
import { base64UrlEncode, toArrayBuffer } from "../../src/encoding.ts";
import {
  encryptWebPush,
  generateUserAgentKeys,
  generateVapidKeys,
  vapidAuthorization,
  type UserAgentKeys,
  type VapidKeys,
} from "../../tools/web-push.ts";
import { createSqliteD1 } from "./sqlite-d1.ts";

export const ORIGIN = "https://push.relay.test";
export const START = 1_800_000_000;

export class MockSender implements PushSender {
  readonly requests: DeliveryRequest[] = [];
  private readonly queued: DeliveryOutcome[] = [];
  private fallback: DeliveryOutcome = { kind: "delivered" };
  /** Runs inside send(), before the outcome is returned (to stage races). */
  during: ((request: DeliveryRequest) => Promise<void>) | null = null;

  next(outcome: DeliveryOutcome): this {
    this.queued.push(outcome);
    return this;
  }

  always(outcome: DeliveryOutcome): this {
    this.fallback = outcome;
    return this;
  }

  async send(request: DeliveryRequest): Promise<DeliveryOutcome> {
    this.requests.push(request);
    if (this.during) await this.during(request);
    return this.queued.shift() ?? this.fallback;
  }
}

export class CapturingLog implements RelayLog {
  readonly lines: string[] = [];
  private record(level: string, event: string, fields?: LogFields) {
    this.lines.push(JSON.stringify({ level, event, ...fields }));
  }
  info(event: string, fields?: LogFields) {
    this.record("info", event, fields);
  }
  warn(event: string, fields?: LogFields) {
    this.record("warn", event, fields);
  }
  error(event: string, fields?: LogFields) {
    this.record("error", event, fields);
  }
  text(): string {
    return this.lines.join("\n");
  }
}

export interface TestRelay {
  relay: ReturnType<typeof createRelay>;
  store: RelayStore;
  sqlite: Database;
  log: CapturingLog;
  clock: { now: number };
  apns: MockSender;
  fcm: MockSender;
  call(method: string, path: string, init?: { body?: unknown; headers?: Record<string, string> }): Promise<Response>;
}

export function createTestRelay(
  options: { limits?: Partial<RelayLimits>; senders?: Partial<Record<Platform, PushSender | null>>; origin?: string | null } = {},
): TestRelay {
  const { db, sqlite } = createSqliteD1();
  const store = new RelayStore(db);
  const log = new CapturingLog();
  const clock = { now: START };
  const apns = new MockSender();
  const fcm = new MockSender();
  const relay = createRelay({
    store,
    counters: new D1Counters(db),
    senders: { apns, fcm, ...options.senders },
    origin: options.origin === undefined ? ORIGIN : options.origin,
    limits: { ...DEFAULT_LIMITS, ...options.limits },
    log,
    now: () => clock.now,
  });
  return {
    relay,
    store,
    sqlite,
    log,
    clock,
    apns,
    fcm,
    call(method, path, init = {}) {
      const headers: Record<string, string> = { ...init.headers };
      let body: BodyInit | undefined;
      if (init.body !== undefined) {
        if (init.body instanceof Uint8Array) {
          body = toArrayBuffer(init.body);
        } else {
          body = JSON.stringify(init.body);
          headers["content-type"] ??= "application/json";
        }
      }
      return relay.fetch(new Request(`${ORIGIN}${path}`, { method, headers, body }));
    },
  };
}

export const APNS_TOKEN = "a1".repeat(32);
export const FCM_TOKEN = "fcm-token_abcdefghijklmnopqrstuvwxyz:0123456789";

export interface RegisteredDevice {
  deviceId: string;
  secret: string;
  auth: Record<string, string>;
}

let tokens = 0;
/** A distinct FCM token per call: registering a token again supersedes its holder. */
export function uniqueFcmToken(): string {
  tokens += 1;
  return `${FCM_TOKEN}-${tokens}`;
}

export async function register(
  t: TestRelay,
  body: Record<string, unknown> = { platform: "fcm", token: uniqueFcmToken() },
  headers: Record<string, string> = {},
): Promise<RegisteredDevice> {
  const response = await t.call("POST", "/v1/devices", { body, headers });
  if (response.status !== 201) throw new Error(`register failed: ${response.status} ${await response.text()}`);
  const json = (await response.json()) as { device_id: string; device_secret: string };
  return {
    deviceId: json.device_id,
    secret: json.device_secret,
    auth: { authorization: `Bearer ${json.device_secret}` },
  };
}

export interface Subscribed {
  subscriptionId: string;
  endpoint: string;
  path: string;
  vapid: VapidKeys;
  userAgent: UserAgentKeys;
}

export async function subscribe(t: TestRelay, device: RegisteredDevice, vapid?: VapidKeys): Promise<Subscribed> {
  const keys = vapid ?? (await generateVapidKeys());
  const response = await t.call("POST", `/v1/devices/${device.deviceId}/subscriptions`, {
    headers: device.auth,
    body: { application_server_key: base64UrlEncode(keys.publicKey) },
  });
  if (response.status !== 201) throw new Error(`subscribe failed: ${response.status} ${await response.text()}`);
  const json = (await response.json()) as { subscription_id: string; endpoint: string };
  return {
    subscriptionId: json.subscription_id,
    endpoint: json.endpoint,
    path: new URL(json.endpoint).pathname,
    vapid: keys,
    userAgent: await generateUserAgentKeys(),
  };
}

export const SECRET_PAYLOAD = "secret notification text that must never be logged";
export const VAPID_SUBJECT = "mailto:operator-who-must-not-be-logged@example.com";

/** Builds the headers and body an application server would send. */
export async function pushInit(
  t: TestRelay,
  subscription: Subscribed,
  overrides: {
    headers?: Record<string, string | null>;
    body?: Uint8Array;
    vapid?: VapidKeys;
    audience?: string;
    expiresAt?: number;
  } = {},
): Promise<{ headers: Record<string, string>; body: Uint8Array }> {
  const body =
    overrides.body ??
    (await encryptWebPush({
      uaPublic: subscription.userAgent.publicKey,
      authSecret: subscription.userAgent.authSecret,
      plaintext: new TextEncoder().encode(SECRET_PAYLOAD),
    }));
  const authorization = await vapidAuthorization(overrides.vapid ?? subscription.vapid, overrides.audience ?? ORIGIN, {
    subject: VAPID_SUBJECT,
    expiresAt: overrides.expiresAt ?? t.clock.now + 12 * 3600,
  });
  const headers: Record<string, string | null> = {
    authorization,
    "content-encoding": "aes128gcm",
    ttl: "3600",
    ...overrides.headers,
  };
  return {
    headers: Object.fromEntries(Object.entries(headers).filter((entry): entry is [string, string] => entry[1] !== null)),
    body,
  };
}

export async function push(
  t: TestRelay,
  subscription: Subscribed,
  overrides: Parameters<typeof pushInit>[2] = {},
): Promise<Response> {
  const init = await pushInit(t, subscription, overrides);
  return t.call("POST", subscription.path, init);
}
