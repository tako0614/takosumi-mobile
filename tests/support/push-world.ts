/**
 * A small world for Takosumi Mobile's own notifications: the real push relay
 * (relay/, in-process on SQLite, with a capturing FCM sender), a fake
 * Takosumi instance serving `/api/v1/account/notification-pushers` and
 * sending standard Web Push, and a fake native plugin that keeps WebCrypto
 * keys and decrypts what the relay delivers, the way the Android plugin does.
 */

import type { MobileSession } from "@takosjp/mobile-kit";
import { base64UrlDecode, base64UrlEncode } from "../../relay/src/encoding.ts";
import {
  decryptWebPush,
  generateVapidKeys,
  sendWebPush,
  type VapidKeys,
} from "../../relay/tools/web-push.ts";
import { createTestRelay, type TestRelay } from "../../relay/tests/support/relay.ts";
import type {
  NativeSubscription,
  NotificationReceived,
  NotificationTap,
  PushNative,
  PushPermissionState,
  PushToken,
  SubscriptionKeyRequest,
} from "../../src/push-native.ts";
import { PushUnavailableError } from "../../src/push-native.ts";

export const RELAY_ORIGIN = "https://push.relay.test";

function memoryValues() {
  const values = new Map<string, string>();
  return {
    values,
    async get(key: string) {
      return values.get(key);
    },
    async set(key: string, value: string) {
      values.set(key, value);
    },
    async delete(key: string) {
      values.delete(key);
    },
  };
}

/** Ordinary persistence (`NativeBridge.storage`). */
export function memoryStore() {
  return { kind: "device-persistent" as const, ...memoryValues() };
}

/** A secure store (`NativeBridge.secureStore`). */
export function memorySecureStore() {
  return { kind: "secure" as const, ...memoryValues() };
}

interface FakeKey {
  readonly request: SubscriptionKeyRequest;
  readonly privateKey: CryptoKey;
  readonly publicKey: Uint8Array;
  readonly auth: Uint8Array;
}

/** What the fake device showed for one delivered message. */
export interface ShownNotification {
  readonly subscriptionId: string;
  readonly instance: string;
  readonly account: string;
  readonly title: string;
  readonly body?: string;
  readonly navigate?: string;
  /** True for generic text (unknown subscription or unreadable message). */
  readonly generic: boolean;
}

export function createFakeNative(options: { permission?: PushPermissionState; grant?: boolean; token?: PushToken | "unavailable" } = {}) {
  let permission: PushPermissionState = options.permission ?? "granted";
  let token: PushToken | "unavailable" = options.token ?? {
    token: "fcm-token_device-one:abcdefghijklmnop",
    provider: "fcm",
    environment: "production",
  };
  const keys = new Map<string, FakeKey>();
  const tapHandlers = new Set<(tap: NotificationTap) => void>();
  const tokenHandlers = new Set<(token: PushToken) => void>();
  let pendingTap: NotificationTap | undefined;
  const native: PushNative = {
    async permissionState() {
      return permission;
    },
    async requestPermission() {
      if (options.grant === false) {
        permission = "denied";
        return false;
      }
      permission = "granted";
      return true;
    },
    async getToken() {
      if (token === "unavailable") throw new PushUnavailableError();
      return token;
    },
    async deviceName() {
      return "Pixel Test";
    },
    async createSubscriptionKeys(request) {
      if (keys.has(request.subscriptionId)) throw new Error("exists");
      const pair = (await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, true, [
        "deriveBits",
      ])) as CryptoKeyPair;
      const publicKey = new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey));
      const auth = crypto.getRandomValues(new Uint8Array(16));
      keys.set(request.subscriptionId, { request: { ...request, origins: [...request.origins] }, privateKey: pair.privateKey, publicKey, auth });
      return { p256dh: base64UrlEncode(publicKey), auth: base64UrlEncode(auth) };
    },
    async deleteSubscriptionKeys(subscriptionId) {
      keys.delete(subscriptionId);
    },
    async listSubscriptions(): Promise<readonly NativeSubscription[]> {
      return [...keys.values()].map((key) => ({
        subscriptionId: key.request.subscriptionId,
        instance: key.request.instance,
        account: key.request.account,
        label: key.request.label,
      }));
    },
    async takePendingTap() {
      const tap = pendingTap;
      pendingTap = undefined;
      return tap;
    },
    async onTap(handler) {
      tapHandlers.add(handler);
      return async () => void tapHandlers.delete(handler);
    },
    async onReceived(_handler: (event: NotificationReceived) => void) {
      return async () => undefined;
    },
    async onTokenRefresh(handler) {
      tokenHandlers.add(handler);
      return async () => void tokenHandlers.delete(handler);
    },
  };
  return {
    native,
    keys,
    setPermission(next: PushPermissionState) {
      permission = next;
    },
    setToken(next: PushToken) {
      token = next;
      for (const handler of tokenHandlers) handler(next);
    },
    /**
     * What the Android plugin does with a relay data message `{s, b}`: find
     * the subscription, decrypt (RFC 8291), read the Declarative Web Push
     * message, keep `navigate` only on the subscription's origins.
     */
    async receive(data: { s: string; b: string }): Promise<ShownNotification> {
      const key = keys.get(data.s);
      const generic = { subscriptionId: data.s, instance: key?.request.instance ?? "", account: key?.request.account ?? "", title: "Takosumi", generic: true };
      if (!key) return generic;
      try {
        const body = base64UrlDecode(data.b);
        if (!body) return generic;
        const plaintext = await decryptWebPush({ uaPrivate: key.privateKey, uaPublic: key.publicKey, authSecret: key.auth, body });
        const message = JSON.parse(new TextDecoder().decode(plaintext)) as { web_push?: unknown; notification?: Record<string, unknown> };
        const notification = message.notification;
        if (message.web_push !== 8030 || typeof notification?.title !== "string") return generic;
        let navigate: string | undefined;
        if (typeof notification.navigate === "string") {
          try {
            const url = new URL(notification.navigate);
            if (!url.username && !url.password && key.request.origins.includes(url.origin)) navigate = notification.navigate;
          } catch {
            navigate = undefined;
          }
        }
        return {
          subscriptionId: data.s,
          instance: key.request.instance,
          account: key.request.account,
          title: notification.title,
          ...(typeof notification.body === "string" ? { body: notification.body } : {}),
          ...(navigate ? { navigate } : {}),
          generic: false,
        };
      } catch {
        return generic;
      }
    },
    /** A tap on a shown notification, as the plugin hands it to the shell. */
    tap(shown: ShownNotification): NotificationTap {
      const tap: NotificationTap = {
        id: crypto.randomUUID(),
        subscriptionId: shown.subscriptionId,
        instance: shown.instance,
        account: shown.account,
        ...(shown.navigate ? { navigate: shown.navigate } : {}),
      };
      pendingTap = tap;
      for (const handler of tapHandlers) handler(tap);
      return tap;
    },
  };
}

export type FakeNative = ReturnType<typeof createFakeNative>;

interface FakePusher {
  id: string;
  app_id: string;
  pushkey: string;
  data: { url: string; auth: string };
  device_display_name: string;
  lang: string;
}

/** A Takosumi instance: its pusher API and its Web Push sender. */
export function createFakeInstance(options: {
  readonly origin: string;
  readonly relayOrigins?: readonly string[];
  readonly unavailable?: "secret_not_configured" | "relay_not_configured" | "issuer_not_https";
  readonly token?: string;
}) {
  let vapid: VapidKeys | undefined;
  const pushers: FakePusher[] = [];
  const requests: { method: string; path: string; body?: unknown }[] = [];
  let counter = 0;
  const relayOrigins = options.relayOrigins ?? [RELAY_ORIGIN];
  const token = options.token ?? "access-token-1";
  const keys = async () => (vapid ??= await generateVapidKeys());
  const error = (status: number, code: string, details?: Record<string, unknown>) =>
    new Response(JSON.stringify({ error: { code, message: code, requestId: "req", ...(details ? { details } : {}) } }), {
      status,
      headers: { "content-type": "application/json" },
    });
  const wire = (pusher: FakePusher) => ({
    id: pusher.id,
    kind: "webpush",
    app_id: pusher.app_id,
    pushkey: pusher.pushkey,
    data: { url: pusher.data.url },
    device_display_name: pusher.device_display_name,
    lang: pusher.lang,
    created_at: "2027-01-01T00:00:00.000Z",
    updated_at: "2027-01-01T00:00:00.000Z",
    last_success_at: null,
    last_failure_at: null,
    last_failure_status: null,
    failing: false,
  });
  return {
    origin: options.origin,
    pushers,
    requests,
    session(accountId = "acct_1", overrides: Partial<MobileSession> = {}): MobileSession {
      return {
        hostUrl: options.origin,
        product: "takosumi",
        oidcIssuer: options.origin,
        oidcClientId: "takosumi-mobile",
        accountId,
        accountLabel: `${accountId}@example`,
        accessToken: token,
        tokenType: "Bearer",
        createdAt: "2027-01-01T00:00:00.000Z",
        ...overrides,
      };
    },
    async rotateKey() {
      vapid = await generateVapidKeys();
    },
    async handle(url: URL, init: RequestInit | undefined): Promise<Response> {
      const method = init?.method ?? "GET";
      const body = typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
      requests.push({ method, path: url.pathname, ...(body === undefined ? {} : { body }) });
      const headers = new Headers(init?.headers);
      if (headers.get("authorization") !== `Bearer ${token}`) return error(401, "unauthorized");
      const base = "/api/v1/account/notification-pushers";
      if (url.pathname === base && method === "GET") {
        const available = !options.unavailable;
        return Response.json({
          pushers: pushers.map(wire),
          web_push_public_key: available ? base64UrlEncode((await keys()).publicKey) : null,
          web_push_unavailable_reason: available ? null : options.unavailable,
        });
      }
      if (url.pathname === base && method === "POST") {
        if (options.unavailable === "relay_not_configured") return error(409, "notification_relay_not_configured");
        const registration = body as FakePusher & { kind: string };
        if (registration.kind !== "webpush" || registration.app_id !== "com.takosumi") return error(400, "invalid_request");
        if (base64UrlDecode(registration.pushkey)?.length !== 65) return error(400, "invalid_request");
        if (base64UrlDecode(registration.data.auth)?.length !== 16) return error(400, "invalid_request");
        if (!relayOrigins.includes(new URL(registration.data.url).origin)) {
          return error(400, "invalid_pusher_endpoint", { reason: "origin_not_allowed" });
        }
        if (pushers.length >= 20) return error(409, "notification_pusher_limit_reached");
        const existing = pushers.find((pusher) => pusher.pushkey === registration.pushkey);
        if (existing) {
          existing.data = registration.data;
          return Response.json({ pusher: wire(existing) });
        }
        counter += 1;
        const pusher: FakePusher = {
          id: `npsh_${String(counter).padStart(32, "0")}`,
          app_id: registration.app_id,
          pushkey: registration.pushkey,
          data: registration.data,
          device_display_name: registration.device_display_name,
          lang: registration.lang,
        };
        pushers.push(pusher);
        return Response.json({ pusher: wire(pusher) }, { status: 201 });
      }
      if (url.pathname.startsWith(`${base}/`) && method === "DELETE") {
        const id = url.pathname.slice(base.length + 1);
        const index = pushers.findIndex((pusher) => pusher.id === id);
        if (index < 0) return error(404, "not_found");
        pushers.splice(index, 1);
        return new Response(null, { status: 204 });
      }
      return error(404, "not_found");
    },
    /** Sends one Declarative Web Push message to a pusher, as the instance does. */
    async send(
      world: PushWorld,
      pusher: FakePusher,
      notification: Record<string, unknown>,
    ): Promise<Response> {
      const response = await sendWebPush({
        endpoint: pusher.data.url,
        vapid: await keys(),
        userAgent: { publicKey: base64UrlDecode(pusher.pushkey)!, authSecret: base64UrlDecode(pusher.data.auth)! },
        payload: new TextEncoder().encode(JSON.stringify({ web_push: 8030, notification })),
        ttl: 86_400,
        urgency: "high",
        subject: options.origin,
        now: world.relay.clock.now,
        fetcher: world.fetch,
      });
      if (response.status === 404 || response.status === 410) {
        pushers.splice(pushers.indexOf(pusher), 1);
      }
      return response;
    },
  };
}

export type FakeInstance = ReturnType<typeof createFakeInstance>;

export interface PushWorld {
  readonly relay: TestRelay;
  readonly fetch: (input: string, init?: RequestInit) => Promise<Response>;
  /** The last data message the relay handed to FCM. */
  readonly lastDelivery: () => { s: string; b: string } | undefined;
}

/** Routes requests to the in-process relay and the fake instances. */
export function createPushWorld(instances: readonly FakeInstance[]): PushWorld {
  const relay = createTestRelay();
  const fetcher = async (input: string, init?: RequestInit): Promise<Response> => {
    const url = new URL(input);
    if (url.origin === RELAY_ORIGIN) return await relay.relay.fetch(new Request(input, init));
    const instance = instances.find((candidate) => candidate.origin === url.origin);
    if (instance) return await instance.handle(url, init);
    throw new Error(`unexpected request to ${input}`);
  };
  return {
    relay,
    fetch: fetcher,
    lastDelivery() {
      const request = relay.fcm.requests.at(-1);
      return request ? { s: request.subscriptionId, b: base64UrlEncode(request.body) } : undefined;
    },
  };
}
