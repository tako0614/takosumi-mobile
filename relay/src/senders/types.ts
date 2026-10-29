import { base64UrlEncode } from "../encoding.ts";
import type { ApnsEnvironment, Platform } from "../store.ts";

export type Urgency = "very-low" | "low" | "normal" | "high";

/**
 * The largest encrypted Web Push body the relay accepts. Base64url makes it
 * 4/3 larger, and APNs (4096 bytes for the whole JSON payload) and FCM (4096
 * bytes for data keys and values) must still fit it with their envelope. The
 * sender tests check both payloads at exactly this size.
 */
export const MAX_PUSH_BODY_BYTES = 2800;

/** What one push asks APNs or FCM to deliver. */
export interface DeliveryRequest {
  platform: Platform;
  token: string;
  apnsEnvironment: ApnsEnvironment | null;
  subscriptionId: string;
  /** The encrypted aes128gcm body exactly as the application server sent it. */
  body: Uint8Array;
  /** Seconds the message may wait for the device (already clamped). */
  ttl: number;
  urgency: Urgency;
  /** RFC 8030 Topic: messages with the same topic replace each other. */
  topic: string | null;
  /** Unix seconds. */
  now: number;
}

export type DeliveryOutcome =
  /** APNs/FCM accepted the message. */
  | { kind: "delivered" }
  /**
   * APNs/FCM says the token is not valid for this app. `expired`: a token that
   * was once registered is not any more (the app was uninstalled or the token
   * expired); platforms answer this only for tokens they issued. `rejected`: the
   * platform does not accept the token at all (malformed, wrong environment).
   */
  | { kind: "device-invalid"; reason: string; evidence: "expired" | "rejected" }
  /** APNs/FCM asked us to slow down. */
  | { kind: "throttled"; retryAfter: number; reason: string }
  /** A transient upstream failure (network, 5xx). */
  | { kind: "unavailable"; retryAfter: number | null; reason: string }
  /** Anything else: our credentials, the payload, or an unknown answer. */
  | { kind: "failed"; reason: string };

/**
 * Delivers to one platform. Implementations must not log the body, the token,
 * or credentials; a `reason` is a short upstream code such as `BadDeviceToken`.
 */
export interface PushSender {
  send(request: DeliveryRequest): Promise<DeliveryOutcome>;
}

export type FetchLike = (input: string, init?: RequestInit) => Promise<Response>;

/**
 * The only data the relay forwards: which subscription the message is for, and
 * the still-encrypted body. The device looks the subscription up in its own key
 * store and decrypts.
 */
export function relayData(request: DeliveryRequest): { s: string; b: string } {
  return { s: request.subscriptionId, b: base64UrlEncode(request.body) };
}

export function retryAfterSeconds(response: Response): number | null {
  const header = response.headers.get("retry-after");
  if (!header) return null;
  const seconds = Number(header);
  if (Number.isFinite(seconds) && seconds >= 0) return Math.ceil(seconds);
  const date = Date.parse(header);
  if (Number.isNaN(date)) return null;
  return Math.max(0, Math.ceil((date - Date.now()) / 1000));
}
