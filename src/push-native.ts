/**
 * The `takosumi-push` native plugin (src-tauri/plugins/push): the APNs/FCM
 * token, one Web Push key pair per Takosumi instance subscription, and the
 * taps on posted notifications. Private keys stay native; this side only sees
 * the public half to register with the instance.
 */

export type PushPermissionState = "granted" | "denied" | "prompt";

export interface PushToken {
  readonly token: string;
  readonly provider: "fcm" | "apns";
  /** `development` is the APNs sandbox. */
  readonly environment: "production" | "development";
}

export interface SubscriptionKeyRequest {
  readonly subscriptionId: string;
  /** The instance origin the subscription belongs to. */
  readonly instance: string;
  /** The account subject, or "" for a session without one. */
  readonly account: string;
  /** The notification channel's name. */
  readonly label: string;
  /** Origins a notification may open: the instance and its Accounts issuer. */
  readonly origins: readonly string[];
}

/** Unpadded base64url: `p256dh` is the 65-byte point, `auth` 16 bytes. */
export interface SubscriptionKeys {
  readonly p256dh: string;
  readonly auth: string;
}

export interface NativeSubscription {
  readonly subscriptionId: string;
  readonly instance: string;
  readonly account: string;
  readonly label: string;
}

/** A tapped notification; `navigate` only when the message could be read. */
export interface NotificationTap {
  readonly id: string;
  readonly subscriptionId: string;
  readonly instance: string;
  readonly account: string;
  readonly navigate?: string;
}

export interface NotificationReceived {
  readonly subscriptionId: string;
  readonly instance: string;
  readonly account: string;
}

export type Unlisten = () => Promise<void>;

export interface PushNative {
  readonly permissionState: () => Promise<PushPermissionState>;
  readonly requestPermission: () => Promise<boolean>;
  /** Throws {@link PushUnavailableError} when this build or device has no push. */
  readonly getToken: () => Promise<PushToken>;
  readonly deviceName: () => Promise<string>;
  readonly createSubscriptionKeys: (request: SubscriptionKeyRequest) => Promise<SubscriptionKeys>;
  readonly deleteSubscriptionKeys: (subscriptionId: string) => Promise<void>;
  readonly listSubscriptions: () => Promise<readonly NativeSubscription[]>;
  readonly takePendingTap: () => Promise<NotificationTap | undefined>;
  readonly onTap: (handler: (tap: NotificationTap) => void) => Promise<Unlisten>;
  readonly onReceived: (handler: (event: NotificationReceived) => void) => Promise<Unlisten>;
  readonly onTokenRefresh: (handler: (token: PushToken) => void) => Promise<Unlisten>;
}

/** No FCM/APNs here: no Firebase configuration, no entitlement, no Play services. */
export class PushUnavailableError extends Error {
  constructor(message = "このTakosumi Mobileでは通知を受け取れません。") {
    super(message);
    this.name = "PushUnavailableError";
  }
}

export type PushInvoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
export type PushListen = <T>(
  plugin: string,
  event: string,
  handler: (payload: T) => void,
) => Promise<{ readonly unregister: () => Promise<void> }>;

const PLUGIN = "takosumi-push";
const command = (name: string) => `plugin:${PLUGIN}|${name}`;
const SUBSCRIPTION_ID = /^[A-Za-z0-9_-]{8,128}$/u;
const BASE64URL = /^[A-Za-z0-9_-]+$/u;

export function createTauriPushNative(adapters: {
  readonly invoke: PushInvoke;
  readonly listen: PushListen;
}): PushNative {
  const listen = async <T>(event: string, decode: (value: unknown) => T | undefined, handler: (value: T) => void) => {
    const listener = await adapters.listen<unknown>(PLUGIN, event, (payload) => {
      const value = decode(payload);
      if (value !== undefined) handler(value);
    });
    return async () => await listener.unregister();
  };
  return {
    async permissionState() {
      const result = record(await adapters.invoke<unknown>(command("permission_state")));
      const state = result?.state;
      return state === "granted" || state === "denied" ? state : "prompt";
    },
    async requestPermission() {
      return record(await adapters.invoke<unknown>(command("request_permission")))?.granted === true;
    },
    async getToken() {
      let result: unknown;
      try {
        result = await adapters.invoke<unknown>(command("get_token"));
      } catch (error) {
        if (String(error).includes("push_unavailable")) throw new PushUnavailableError();
        throw error;
      }
      const token = decodeToken(result);
      if (!token) throw new Error("The native push token is invalid.");
      return token;
    },
    async deviceName() {
      const name = record(await adapters.invoke<unknown>(command("device_name")))?.name;
      return typeof name === "string" && name.trim() ? name.trim().slice(0, 128) : "Takosumi Mobile";
    },
    async createSubscriptionKeys(request) {
      const result = record(
        await adapters.invoke<unknown>(command("create_subscription_keys"), {
          payload: { ...request, origins: [...request.origins] },
        }),
      );
      const p256dh = result?.p256dh;
      const auth = result?.auth;
      if (!isBase64Url(p256dh, 87) || !isBase64Url(auth, 22)) {
        throw new Error("The native subscription keys are invalid.");
      }
      return { p256dh, auth };
    },
    async deleteSubscriptionKeys(subscriptionId) {
      await adapters.invoke(command("delete_subscription_keys"), { payload: { subscriptionId } });
    },
    async listSubscriptions() {
      const list = record(await adapters.invoke<unknown>(command("list_subscriptions")))?.subscriptions;
      return Array.isArray(list) ? list.map(decodeSubscription).filter((item) => item !== undefined) : [];
    },
    async takePendingTap() {
      return decodeTap(record(await adapters.invoke<unknown>(command("take_pending_tap")))?.tap);
    },
    onTap: (handler) => listen("notification-tapped", decodeTap, handler),
    onReceived: (handler) => listen("notification-received", decodeSubscription, handler),
    onTokenRefresh: (handler) => listen("token-refresh", decodeToken, handler),
  };
}

export function decodeToken(value: unknown): PushToken | undefined {
  const body = record(value);
  if (!body) return undefined;
  const token = typeof body.token === "string" ? body.token.trim() : "";
  if (!token || token.length > 4096) return undefined;
  if (body.provider !== "fcm" && body.provider !== "apns") return undefined;
  const environment = body.environment === "development" ? "development" : "production";
  return { token, provider: body.provider, environment };
}

export function decodeTap(value: unknown): NotificationTap | undefined {
  const body = record(value);
  if (!body) return undefined;
  const subscription = decodeSubscription({ ...body, label: "tap" });
  const id = typeof body.id === "string" ? body.id : "";
  if (!subscription || !id || id.length > 128) return undefined;
  const navigate = typeof body.navigate === "string" && body.navigate.length <= 2048 ? body.navigate : undefined;
  return {
    id,
    subscriptionId: subscription.subscriptionId,
    instance: subscription.instance,
    account: subscription.account,
    ...(navigate ? { navigate } : {}),
  };
}

function decodeSubscription(value: unknown): NativeSubscription | undefined {
  const body = record(value);
  if (!body) return undefined;
  const { subscriptionId, instance, account, label } = body;
  if (typeof subscriptionId !== "string" || !SUBSCRIPTION_ID.test(subscriptionId)) return undefined;
  if (typeof instance !== "string" || !instance) return undefined;
  return {
    subscriptionId,
    instance,
    account: typeof account === "string" ? account : "",
    label: typeof label === "string" ? label : "",
  };
}

function isBase64Url(value: unknown, length: number): value is string {
  return typeof value === "string" && value.length === length && BASE64URL.test(value);
}

function record(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}
