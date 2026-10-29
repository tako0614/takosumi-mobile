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
  };
}

const DEVICE_COLUMNS =
  "d.device_id, d.secret_hash, d.platform, d.token, d.apns_environment, d.invalidated_at, d.created_at, d.updated_at";

/** D1-backed storage for devices and subscriptions. Times are Unix seconds. */
export class RelayStore {
  constructor(private readonly db: D1Database) {}

  async createDevice(device: Omit<DeviceRecord, "invalidatedAt" | "updatedAt">) {
    await this.db
      .prepare(
        "INSERT INTO devices (device_id, secret_hash, platform, token, apns_environment, invalidated_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?6)",
      )
      .bind(
        device.deviceId,
        device.secretHash,
        device.platform,
        device.token,
        device.apnsEnvironment,
        device.createdAt,
      )
      .run();
  }

  async getDevice(deviceId: string): Promise<DeviceRecord | null> {
    const row = await this.db
      .prepare(`SELECT ${DEVICE_COLUMNS} FROM devices d WHERE d.device_id = ?1`)
      .bind(deviceId)
      .first<DeviceRow>();
    return row ? toDevice(row) : null;
  }

  /** Replaces the token. A new token also clears an earlier invalidation. */
  async updateDeviceToken(
    deviceId: string,
    token: string,
    apnsEnvironment: ApnsEnvironment | null,
    now: number,
  ) {
    await this.db
      .prepare(
        "UPDATE devices SET token = ?2, apns_environment = ?3, invalidated_at = NULL, updated_at = ?4 WHERE device_id = ?1",
      )
      .bind(deviceId, token, apnsEnvironment, now)
      .run();
  }

  async invalidateDevice(deviceId: string, now: number) {
    await this.db
      .prepare(
        "UPDATE devices SET invalidated_at = ?2, updated_at = ?2 WHERE device_id = ?1 AND invalidated_at IS NULL",
      )
      .bind(deviceId, now)
      .run();
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
   * Removes devices whose token was reported invalid before `invalidBefore`
   * (with their subscriptions; their endpoints then answer 404), and rate-limit
   * windows that started before `windowsBefore`.
   */
  async purge(invalidBefore: number, windowsBefore: number) {
    await this.db.batch([
      this.db
        .prepare(
          "DELETE FROM subscriptions WHERE device_id IN (SELECT device_id FROM devices WHERE invalidated_at IS NOT NULL AND invalidated_at < ?1)",
        )
        .bind(invalidBefore),
      this.db
        .prepare("DELETE FROM devices WHERE invalidated_at IS NOT NULL AND invalidated_at < ?1")
        .bind(invalidBefore),
      this.db.prepare("DELETE FROM rate_windows WHERE window_start < ?1").bind(windowsBefore),
    ]);
  }
}
