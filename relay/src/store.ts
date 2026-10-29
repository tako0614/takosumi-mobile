import type { D1Database } from "./d1.ts";

export type Platform = "apns" | "fcm";
export type ApnsEnvironment = "production" | "development";

export interface DeviceRecord {
  deviceId: string;
  secretHash: string;
  platform: Platform;
  token: string;
  apnsEnvironment: ApnsEnvironment | null;
  invalidatedAt: number | null;
  createdAt: number;
  updatedAt: number;
  seenAt: number;
  /** When a push to the device last succeeded (written at most daily), or null. */
  deliveredAt: number | null;
}

export interface SubscriptionTarget {
  subscriptionId: string;
  endpointId: string;
  applicationServerKey: string;
  device: DeviceRecord;
}

interface DeviceRow {
  device_id: string;
  secret_hash: string;
  platform: Platform;
  token: string;
  apns_environment: ApnsEnvironment | null;
  invalidated_at: number | null;
  created_at: number;
  updated_at: number;
  seen_at: number;
  delivered_at: number | null;
}

interface TargetRow extends DeviceRow {
  subscription_id: string;
  endpoint_id: string;
  application_server_key: string;
}

function toDevice(row: DeviceRow): DeviceRecord {
  return {
    deviceId: row.device_id,
    secretHash: row.secret_hash,
    platform: row.platform,
    token: row.token,
    apnsEnvironment: row.apns_environment ?? null,
    invalidatedAt: row.invalidated_at ?? null,
    createdAt: row.created_at,
    updatedAt: row.updated_at,
    seenAt: row.seen_at,
    deliveredAt: row.delivered_at ?? null,
  };
}

const DEVICE_COLUMNS =
  "d.device_id, d.secret_hash, d.platform, d.token, d.apns_environment, d.invalidated_at, d.created_at, d.updated_at, d.seen_at, d.delivered_at";

// Marks every other active device holding the token as superseded.
const SUPERSEDE =
  "UPDATE devices SET invalidated_at = ?3, updated_at = ?3 WHERE platform = ?1 AND token = ?2 AND invalidated_at IS NULL AND device_id != ?4 RETURNING device_id";

/** D1-backed storage for devices and subscriptions. Times are Unix seconds. */
export class RelayStore {
  constructor(private readonly db: D1Database) {}

  /**
   * Inserts the device and supersedes any other active device with the same
   * token, in one transaction. Returns how many devices were superseded.
   */
  async createDevice(
    device: Omit<DeviceRecord, "invalidatedAt" | "updatedAt" | "seenAt" | "deliveredAt">,
  ): Promise<number> {
    const [superseded] = await this.db.batch<{ device_id: string }>([
      this.db.prepare(SUPERSEDE).bind(device.platform, device.token, device.createdAt, device.deviceId),
      this.db
        .prepare(
          "INSERT INTO devices (device_id, secret_hash, platform, token, apns_environment, invalidated_at, created_at, updated_at, seen_at) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?6, ?6)",
        )
        .bind(
          device.deviceId,
          device.secretHash,
          device.platform,
          device.token,
          device.apnsEnvironment,
          device.createdAt,
        ),
    ]);
    return superseded?.results.length ?? 0;
  }

  async getDevice(deviceId: string): Promise<DeviceRecord | null> {
    const row = await this.db
      .prepare(`SELECT ${DEVICE_COLUMNS} FROM devices d WHERE d.device_id = ?1`)
      .bind(deviceId)
      .first<DeviceRow>();
    return row ? toDevice(row) : null;
  }

  /**
   * Keeps at most `keep` invalid rows for this token (the newest) and deletes
   * the older ones with their subscriptions; their endpoints then answer 404,
   * which apps treat like 410. Re-registering one token many times therefore
   * cannot grow the table, and never locks the token out.
   */
  async pruneInvalidRowsForToken(platform: Platform, token: string, keep: number) {
    const older =
      "SELECT device_id FROM devices WHERE platform = ?1 AND token = ?2 AND invalidated_at IS NOT NULL ORDER BY invalidated_at DESC, created_at DESC, rowid DESC LIMIT -1 OFFSET ?3";
    await this.db.batch([
      this.db.prepare(`DELETE FROM subscriptions WHERE device_id IN (${older})`).bind(platform, token, keep),
      this.db.prepare(`DELETE FROM devices WHERE device_id IN (${older})`).bind(platform, token, keep),
    ]);
  }

  /**
   * Records a successful delivery: the device is in use. Callers write this at
   * most once a day per device.
   */
  async recordDelivery(deviceId: string, now: number) {
    await this.db
      .prepare("UPDATE devices SET delivered_at = ?2, seen_at = MAX(seen_at, ?2) WHERE device_id = ?1")
      .bind(deviceId, now)
      .run();
  }

  /** Records an authenticated call from the device. */
  async touchDevice(deviceId: string, now: number) {
    await this.db.prepare("UPDATE devices SET seen_at = ?2 WHERE device_id = ?1").bind(deviceId, now).run();
  }

  /**
   * Replaces the token. A new token also clears an earlier invalidation, and
   * supersedes any other active device holding the same token. Returns how many
   * devices were superseded.
   */
  async updateDeviceToken(
    device: Pick<DeviceRecord, "deviceId" | "platform">,
    token: string,
    apnsEnvironment: ApnsEnvironment | null,
    now: number,
  ): Promise<number> {
    const [superseded] = await this.db.batch<{ device_id: string }>([
      this.db.prepare(SUPERSEDE).bind(device.platform, token, now, device.deviceId),
      this.db
        .prepare(
          "UPDATE devices SET token = ?2, apns_environment = ?3, invalidated_at = NULL, updated_at = ?4, seen_at = ?4 WHERE device_id = ?1",
        )
        .bind(device.deviceId, token, apnsEnvironment, now),
    ]);
    return superseded?.results.length ?? 0;
  }

  /**
   * Marks the device invalid, but only while it still holds `token`, the token
   * APNs/FCM rejected. Returns false when nothing changed: the device replaced
   * its token meanwhile, was already invalid, or is gone.
   */
  async invalidateDevice(deviceId: string, token: string, now: number): Promise<boolean> {
    const row = await this.db
      .prepare(
        "UPDATE devices SET invalidated_at = ?3, updated_at = ?3 WHERE device_id = ?1 AND token = ?2 AND invalidated_at IS NULL RETURNING device_id",
      )
      .bind(deviceId, token, now)
      .first<{ device_id: string }>();
    return row !== null;
  }

  /** Deletes the device and every subscription it holds. */
  async deleteDevice(deviceId: string) {
    await this.db.batch([
      this.db.prepare("DELETE FROM subscriptions WHERE device_id = ?1").bind(deviceId),
      this.db.prepare("DELETE FROM devices WHERE device_id = ?1").bind(deviceId),
    ]);
  }

  async countSubscriptions(deviceId: string): Promise<number> {
    const row = await this.db
      .prepare("SELECT COUNT(*) AS n FROM subscriptions WHERE device_id = ?1")
      .bind(deviceId)
      .first<{ n: number }>();
    return Number(row?.n ?? 0);
  }

  async createSubscription(subscription: {
    subscriptionId: string;
    deviceId: string;
    endpointId: string;
    applicationServerKey: string;
    createdAt: number;
  }) {
    await this.db
      .prepare(
        "INSERT INTO subscriptions (subscription_id, device_id, endpoint_id, application_server_key, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
      )
      .bind(
        subscription.subscriptionId,
        subscription.deviceId,
        subscription.endpointId,
        subscription.applicationServerKey,
        subscription.createdAt,
      )
      .run();
  }

  /** Returns false when the device holds no such subscription. */
  async deleteSubscription(deviceId: string, subscriptionId: string): Promise<boolean> {
    const row = await this.db
      .prepare(
        "DELETE FROM subscriptions WHERE device_id = ?1 AND subscription_id = ?2 RETURNING subscription_id",
      )
      .bind(deviceId, subscriptionId)
      .first<{ subscription_id: string }>();
    return row !== null;
  }

  async findByEndpoint(endpointId: string): Promise<SubscriptionTarget | null> {
    const row = await this.db
      .prepare(
        `SELECT s.subscription_id, s.endpoint_id, s.application_server_key, ${DEVICE_COLUMNS} FROM subscriptions s JOIN devices d ON d.device_id = s.device_id WHERE s.endpoint_id = ?1`,
      )
      .bind(endpointId)
      .first<TargetRow>();
    if (!row) return null;
    return {
      subscriptionId: row.subscription_id,
      endpointId: row.endpoint_id,
      applicationServerKey: row.application_server_key,
      device: toDevice(row),
    };
  }

  /**
   * Deletes devices (with their subscriptions; their endpoints then answer 404)
   * that were invalidated before `invalidBefore` or last seen before
   * `unseenBefore`, and counter windows that started before `windowsBefore`.
   */
  async purge(invalidBefore: number, unseenBefore: number, windowsBefore: number) {
    const expired =
      "SELECT device_id FROM devices WHERE (invalidated_at IS NOT NULL AND invalidated_at < ?1) OR seen_at < ?2";
    await this.db.batch([
      this.db
        .prepare(`DELETE FROM subscriptions WHERE device_id IN (${expired})`)
        .bind(invalidBefore, unseenBefore),
      this.db
        .prepare("DELETE FROM devices WHERE (invalidated_at IS NOT NULL AND invalidated_at < ?1) OR seen_at < ?2")
        .bind(invalidBefore, unseenBefore),
      this.db.prepare("DELETE FROM rate_windows WHERE window_start < ?1").bind(windowsBefore),
    ]);
  }
}
