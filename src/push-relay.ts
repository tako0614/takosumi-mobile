/**
 * Client for the push relay's device API (relay/README.md「Device API」). The
 * relay is run by Takosumi Mobile's publisher; its origin is fixed at build
 * time (`VITE_TAKOSUMI_PUSH_RELAY_ORIGIN`) and each instance's operator lists
 * the same origin in `TAKOSUMI_NOTIFICATION_PUSH_RELAY_ORIGINS`.
 */

import type { PushToken } from "./push-native.ts";

export type RelayFetch = (input: string, init?: RequestInit) => Promise<Response>;

/** This device on the relay. `deviceSecret` lives only in the secure store. */
export interface RelayDevice {
  readonly relayOrigin: string;
  readonly deviceId: string;
  readonly deviceSecret: string;
  readonly platform: "fcm" | "apns";
  readonly token: string;
  readonly apnsEnvironment?: "production" | "development";
}

export interface RelaySubscription {
  readonly subscriptionId: string;
  readonly endpoint: string;
}

/** A relay answer other than success; `code` is the relay's `error` field. */
export class RelayError extends Error {
  readonly status: number;
  readonly code: string | undefined;

  constructor(status: number, code: string | undefined, message: string) {
    super(message);
    this.name = "RelayError";
    this.status = status;
    this.code = code;
  }

  /** The device is unknown or its secret is wrong: register again. */
  get deviceUnknown(): boolean {
    return this.status === 401;
  }

  /** The device's token was rejected or superseded: send a new token first. */
  get deviceTokenInvalid(): boolean {
    return this.status === 409 && this.code === "device_token_invalid";
  }
}

const ID = /^[A-Za-z0-9_-]{16,128}$/u;
const SECRET = /^[A-Za-z0-9_-]{16,128}$/u;

/** The relay's https origin (http only on loopback, for a local relay). */
export function normalizeRelayOrigin(value: string | undefined): string | undefined {
  const text = value?.trim();
  if (!text) return undefined;
  try {
    const url = new URL(text);
    const loopback = ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
    if (url.protocol !== "https:" && !(url.protocol === "http:" && loopback)) return undefined;
    if (url.username || url.password || url.search || url.hash) return undefined;
    if (url.pathname !== "/" && url.pathname !== "") return undefined;
    return url.origin;
  } catch {
    return undefined;
  }
}

export function createRelayClient(input: { readonly origin: string; readonly fetch: RelayFetch }) {
  const origin = input.origin;
  const request = async (
    path: string,
    init: { readonly method: string; readonly secret?: string; readonly body?: unknown },
  ): Promise<unknown> => {
    let response: Response;
    try {
      response = await input.fetch(`${origin}${path}`, {
        method: init.method,
        headers: {
          accept: "application/json",
          ...(init.body === undefined ? {} : { "content-type": "application/json" }),
          ...(init.secret ? { authorization: `Bearer ${init.secret}` } : {}),
        },
        ...(init.body === undefined ? {} : { body: JSON.stringify(init.body) }),
      });
    } catch (error) {
      throw new RelayError(0, "unreachable", `The push relay could not be reached: ${String(error)}`);
    }
    if (response.status === 204) return undefined;
    const text = await response.text().catch(() => "");
    let body: unknown;
    try {
      body = text ? JSON.parse(text) : undefined;
    } catch {
      body = undefined;
    }
    if (!response.ok) {
      const code = typeof (body as { error?: unknown })?.error === "string" ? (body as { error: string }).error : undefined;
      throw new RelayError(response.status, code, `The push relay answered ${response.status}${code ? ` ${code}` : ""}.`);
    }
    return body;
  };
  const tokenFields = (token: PushToken) => ({
    token: token.token,
    ...(token.provider === "apns" ? { apns_environment: token.environment } : {}),
  });
  const devicePath = (device: RelayDevice) => `/v1/devices/${encodeURIComponent(device.deviceId)}`;

  return {
    origin,
    async registerDevice(token: PushToken): Promise<RelayDevice> {
      const body = record(
        await request("/v1/devices", { method: "POST", body: { platform: token.provider, ...tokenFields(token) } }),
      );
      const deviceId = body?.device_id;
      const deviceSecret = body?.device_secret;
      if (typeof deviceId !== "string" || !ID.test(deviceId) || typeof deviceSecret !== "string" || !SECRET.test(deviceSecret)) {
        throw new RelayError(502, "malformed", "The push relay's device registration answer is malformed.");
      }
      return {
        relayOrigin: origin,
        deviceId,
        deviceSecret,
        platform: token.provider,
        token: token.token,
        ...(token.provider === "apns" ? { apnsEnvironment: token.environment } : {}),
      };
    },
    async updateDevice(device: RelayDevice, token: PushToken): Promise<RelayDevice> {
      if (token.provider !== device.platform) {
        throw new RelayError(400, "platform_immutable", "A relay device cannot change platform.");
      }
      await request(devicePath(device), { method: "PUT", secret: device.deviceSecret, body: tokenFields(token) });
      return {
        ...device,
        token: token.token,
        ...(token.provider === "apns" ? { apnsEnvironment: token.environment } : {}),
      };
    },
    async deleteDevice(device: RelayDevice): Promise<void> {
      await request(devicePath(device), { method: "DELETE", secret: device.deviceSecret });
    },
    async createSubscription(device: RelayDevice, applicationServerKey: string): Promise<RelaySubscription> {
      const body = record(
        await request(`${devicePath(device)}/subscriptions`, {
          method: "POST",
          secret: device.deviceSecret,
          body: { application_server_key: applicationServerKey },
        }),
      );
      const subscriptionId = body?.subscription_id;
      const endpoint = body?.endpoint;
      if (typeof subscriptionId !== "string" || !ID.test(subscriptionId) || !isRelayEndpoint(endpoint, origin)) {
        throw new RelayError(502, "malformed", "The push relay's subscription answer is malformed.");
      }
      return { subscriptionId, endpoint };
    },
    /** Deletes one subscription; one the relay no longer has counts as deleted. */
    async deleteSubscription(device: RelayDevice, subscriptionId: string): Promise<void> {
      try {
        await request(`${devicePath(device)}/subscriptions/${encodeURIComponent(subscriptionId)}`, {
          method: "DELETE",
          secret: device.deviceSecret,
        });
      } catch (error) {
        if (error instanceof RelayError && error.status === 404) return;
        throw error;
      }
    },
  };
}

export type RelayClient = ReturnType<typeof createRelayClient>;

/** `https://<relay>/push/<id>`: the endpoint the instance will POST to. */
function isRelayEndpoint(value: unknown, origin: string): value is string {
  if (typeof value !== "string" || value.length > 1024) return false;
  try {
    const url = new URL(value);
    return url.origin === origin && /^\/push\/[A-Za-z0-9_-]{16,128}$/u.test(url.pathname) && !url.search && !url.hash;
  } catch {
    return false;
  }
}

function record(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}
