// APNs sender (token-based provider authentication, HTTP/2 provider API).
//
// APNs only speaks HTTP/2. A Worker's fetch() reaches it from Cloudflare's edge
// (observed 2026-10-29: api.push.apple.com and api.sandbox.push.apple.com both
// answered with APNs JSON reasons and an apns-id header), but local workerd
// (`wrangler dev`) speaks HTTP/1.1 and fails with "Network connection lost".
// The sender takes its fetch as a parameter, so it can also run in another
// runtime that has an HTTP/2 client, unchanged, if that ever becomes necessary.

import { importEs256PrivateKey, signEs256 } from "../jwt.ts";
import type { DeliveryOutcome, DeliveryRequest, FetchLike, PushSender } from "./types.ts";
import { relayData, retryAfterSeconds } from "./types.ts";

export interface ApnsConfig {
  teamId: string;
  keyId: string;
  /** The `.p8` key from Apple Developer, PKCS#8 PEM. */
  privateKeyPem: string;
  /** The app's bundle id, used as apns-topic. */
  topic: string;
  /** Shown only if the Notification Service Extension cannot run. */
  placeholderTitle?: string;
  placeholderBody?: string;
}

export const APNS_HOSTS = {
  production: "https://api.push.apple.com",
  development: "https://api.sandbox.push.apple.com",
} as const;

// APNs rejects provider tokens older than an hour and asks providers not to mint
// new ones more often than every 20 minutes. Every isolate derives `iat` from
// the same 40-minute bucket, so concurrent isolates present tokens with the same
// issue time, and no token is ever older than 40 minutes.
const TOKEN_BUCKET_SECONDS = 40 * 60;

// Answers about this one device token. Only these may invalidate a device.
export const APNS_DEVICE_INVALID_REASONS = new Set(["BadDeviceToken", "Unregistered", "ExpiredToken"]);

// Answers about the relay's own configuration: key, team, topic, environment.
// A wrong APNS_TOPIC makes APNs answer DeviceTokenNotForTopic for every device,
// so treating these as "device invalid" would erase every subscription after
// one bad deploy. They are relay failures (502) and never touch the device.
export const APNS_CONFIGURATION_REASONS = new Set([
  "DeviceTokenNotForTopic",
  "TopicDisallowed",
  "MissingTopic",
  "BadTopic",
  "BadCertificate",
  "BadCertificateEnvironment",
  "BadEnvironmentKeyInToken",
  "Forbidden",
  "InvalidProviderToken",
  "ExpiredProviderToken",
  "MissingProviderToken",
  "UnrelatedKeyIdInToken",
]);
const PROVIDER_TOKEN_REASONS = new Set(["ExpiredProviderToken", "InvalidProviderToken", "MissingProviderToken"]);

export function apnsPriority(urgency: DeliveryRequest["urgency"]): "10" | "5" {
  return urgency === "high" || urgency === "normal" ? "10" : "5";
}

export function apnsPayload(request: DeliveryRequest, config: Pick<ApnsConfig, "placeholderTitle" | "placeholderBody">) {
  return {
    aps: {
      alert: {
        title: config.placeholderTitle ?? "Takosumi",
        body: config.placeholderBody ?? "新しい通知",
      },
      "mutable-content": 1,
    },
    ...relayData(request),
  };
}

export class ApnsSender implements PushSender {
  private key: Promise<CryptoKey> | null = null;
  private cached: { iat: number; token: string } | null = null;

  constructor(
    private readonly config: ApnsConfig,
    private readonly fetcher: FetchLike = (input, init) => fetch(input, init),
  ) {}

  private async providerToken(now: number): Promise<string> {
    const iat = now - (now % TOKEN_BUCKET_SECONDS);
    if (this.cached?.iat === iat) return this.cached.token;
    this.key ??= importEs256PrivateKey(this.config.privateKeyPem);
    const token = await signEs256(
      { alg: "ES256", kid: this.config.keyId },
      { iss: this.config.teamId, iat },
      await this.key,
    );
    this.cached = { iat, token };
    return token;
  }

  async send(request: DeliveryRequest): Promise<DeliveryOutcome> {
    let token: string;
    try {
      token = await this.providerToken(request.now);
    } catch {
      return { kind: "failed", reason: "apns-key-unusable" };
    }
    const host = APNS_HOSTS[request.apnsEnvironment ?? "production"];
    const headers: Record<string, string> = {
      authorization: `bearer ${token}`,
      "apns-push-type": "alert",
      "apns-topic": this.config.topic,
      "apns-priority": apnsPriority(request.urgency),
      "apns-expiration": String(request.ttl === 0 ? 0 : request.now + request.ttl),
      "content-type": "application/json",
    };
    if (request.topic) headers["apns-collapse-id"] = request.topic;

    let response: Response;
    try {
      response = await this.fetcher(`${host}/3/device/${encodeURIComponent(request.token)}`, {
        method: "POST",
        headers,
        body: JSON.stringify(apnsPayload(request, this.config)),
      });
    } catch {
      return { kind: "unavailable", retryAfter: null, reason: "apns-unreachable" };
    }
    if (response.status === 200) return { kind: "delivered" };

    let reason = `http-${response.status}`;
    try {
      const parsed = (await response.json()) as { reason?: unknown };
      if (typeof parsed.reason === "string" && /^[A-Za-z]{1,64}$/u.test(parsed.reason)) {
        reason = parsed.reason;
      }
    } catch {
      // Keep the status-derived reason.
    }

    if (APNS_CONFIGURATION_REASONS.has(reason)) {
      if (PROVIDER_TOKEN_REASONS.has(reason)) this.cached = null;
      return { kind: "failed", reason };
    }
    if (APNS_DEVICE_INVALID_REASONS.has(reason) || response.status === 410) {
      return { kind: "device-invalid", reason };
    }
    if (response.status === 429) {
      return { kind: "throttled", retryAfter: retryAfterSeconds(response) ?? 60, reason };
    }
    if (response.status >= 500) {
      return { kind: "unavailable", retryAfter: retryAfterSeconds(response), reason };
    }
    return { kind: "failed", reason };
  }
}
