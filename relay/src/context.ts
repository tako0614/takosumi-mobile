import type { RelayLog } from "./log.ts";
import type { RateLimiter } from "./rate-limit.ts";
import type { PushSender } from "./senders/types.ts";
import type { Platform, RelayStore } from "./store.ts";

export interface RelayLimits {
  /** Pushes per endpoint per minute. */
  endpointPerMinute: number;
  /** Pushes per device (over all its endpoints) per minute. */
  devicePerMinute: number;
  /** Device registrations per client address per hour. */
  registrationsPerHour: number;
  /** Subscriptions created per device per hour. */
  subscriptionsPerHour: number;
  /** Subscriptions a device may hold at once. */
  subscriptionsPerDevice: number;
  /** The largest TTL honored; longer TTLs are reduced and the response says so. */
  maxTtlSeconds: number;
  /** How long a device whose token was reported invalid keeps answering 410. */
  invalidRetentionSeconds: number;
}

export const DEFAULT_LIMITS: RelayLimits = {
  endpointPerMinute: 60,
  devicePerMinute: 240,
  registrationsPerHour: 30,
  subscriptionsPerHour: 120,
  subscriptionsPerDevice: 200,
  maxTtlSeconds: 28 * 24 * 60 * 60,
  invalidRetentionSeconds: 30 * 24 * 60 * 60,
};

export interface RelayContext {
  store: RelayStore;
  rateLimiter: RateLimiter;
  /**
   * The sender per platform. `null` means the platform is not configured here;
   * pushes for its devices answer 503 without touching the device.
   */
  senders: Record<Platform, PushSender | null>;
  /**
   * The relay's public origin. Endpoints are issued under it and VAPID `aud`
   * must equal it. When unset, the origin of the incoming request is used.
   */
  origin: string | null;
  limits: RelayLimits;
  log: RelayLog;
  /** Unix seconds. */
  now(): number;
}
