import type { RelayLog } from "./log.ts";
import type { Counters } from "./rate-limit.ts";
import type { PushSender } from "./senders/types.ts";
import type { Platform, RelayStore } from "./store.ts";

export interface RelayLimits {
  /** Pushes per endpoint per minute. */
  endpointPerMinute: number;
  /**
   * Pushes per device per minute over all its endpoints. Past this ceiling an
   * endpoint may still send up to its fair share (this limit divided by the
   * device's endpoints active in the minute), so busy apps cannot starve others.
   */
  devicePerMinute: number;
  /** Device registrations per client (IPv4 address or IPv6 /64) per hour. */
  registrationsPerHour: number;
  /** Device registrations per IPv6 /48 per hour (a site's whole allocation). */
  registrationsPerHourPer48: number;
  /** Over all clients, registrations past this per hour are logged as an alert. */
  registrationsAlertPerHour: number;
  /** Over all clients, a hard ceiling per hour that only a flood reaches. */
  registrationsPerHourGlobal: number;
  /** Subscriptions created per device per hour. */
  subscriptionsPerHour: number;
  /** Subscriptions a device may hold at once. */
  subscriptionsPerDevice: number;
  /** Invalid (superseded or rejected) rows kept per token; older ones are deleted. */
  invalidRowsPerToken: number;
  /** The largest TTL honored; longer TTLs are reduced and the response says so. */
  maxTtlSeconds: number;
  /** How long a device whose token was reported invalid keeps answering 410. */
  invalidRetentionSeconds: number;
  /** Devices that made no authenticated call for this long are deleted. */
  inactiveDeviceSeconds: number;
  /** The window the invalidation breaker counts in, per platform. */
  breakerWindowSeconds: number;
  /**
   * The breaker counts only `rejected` answers for devices that were delivered
   * to before. It opens when those exceed this in the window...
   */
  breakerMaxInvalidations: number;
  /** ...or exceed this share of deliveries, once there were breakerMinDeliveries. */
  breakerMaxRatio: number;
  breakerMinDeliveries: number;
}

export const DEFAULT_LIMITS: RelayLimits = {
  endpointPerMinute: 60,
  devicePerMinute: 240,
  registrationsPerHour: 30,
  registrationsPerHourPer48: 120,
  registrationsAlertPerHour: 3000,
  registrationsPerHourGlobal: 60000,
  subscriptionsPerHour: 120,
  subscriptionsPerDevice: 64,
  invalidRowsPerToken: 8,
  maxTtlSeconds: 28 * 24 * 60 * 60,
  invalidRetentionSeconds: 30 * 24 * 60 * 60,
  inactiveDeviceSeconds: 180 * 24 * 60 * 60,
  breakerWindowSeconds: 10 * 60,
  breakerMaxInvalidations: 20,
  breakerMaxRatio: 0.25,
  breakerMinDeliveries: 20,
};

export interface RelayContext {
  store: RelayStore;
  counters: Counters;
  /**
   * The sender per platform. `null` means the platform is not configured here;
   * pushes for its devices answer 503 without touching the device.
   */
  senders: Record<Platform, PushSender | null>;
  /**
   * The relay's public origin. Endpoints are issued under it and VAPID `aud`
   * must equal it. Required in production; in staging, when unset, the origin
   * of the incoming request is used.
   */
  origin: string | null;
  limits: RelayLimits;
  log: RelayLog;
  /** Unix seconds. */
  now(): number;
}
