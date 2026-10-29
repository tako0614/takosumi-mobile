// FCM sender (HTTP v1 API, service-account OAuth 2.0).

import { importRs256PrivateKey, signRs256 } from "../jwt.ts";
import type { DeliveryOutcome, DeliveryRequest, FetchLike, PushSender } from "./types.ts";
import { relayData, retryAfterSeconds } from "./types.ts";

export interface FcmServiceAccount {
  client_email: string;
  private_key: string;
}

export interface FcmConfig {
  projectId: string;
  serviceAccount: FcmServiceAccount;
}

export const GOOGLE_TOKEN_URL = "https://oauth2.googleapis.com/token";
export const FCM_SCOPE = "https://www.googleapis.com/auth/firebase.messaging";

/** Parses the service account JSON a Firebase project hands out. */
export function parseServiceAccount(json: string): FcmServiceAccount | null {
  try {
    const value = JSON.parse(json) as Partial<FcmServiceAccount>;
    if (typeof value.client_email === "string" && typeof value.private_key === "string") {
      return { client_email: value.client_email, private_key: value.private_key };
    }
  } catch {
    // Not JSON.
  }
  return null;
}

export function fcmMessage(request: DeliveryRequest) {
  const android: Record<string, string> = {
    priority: request.urgency === "high" ? "HIGH" : "NORMAL",
    ttl: `${request.ttl}s`,
  };
  if (request.topic) android.collapse_key = request.topic;
  return { message: { token: request.token, data: relayData(request), android } };
}

interface FcmErrorBody {
  error?: {
    status?: string;
    message?: string;
    details?: { "@type"?: string; errorCode?: string }[];
  };
}

export class FcmSender implements PushSender {
  private key: Promise<CryptoKey> | null = null;
  private cached: { token: string; expiresAt: number } | null = null;

  constructor(
    private readonly config: FcmConfig,
    private readonly fetcher: FetchLike = (input, init) => fetch(input, init),
  ) {}

  private async accessToken(now: number): Promise<string | DeliveryOutcome> {
    if (this.cached && this.cached.expiresAt - 300 > now) return this.cached.token;
    let assertion: string;
    try {
      this.key ??= importRs256PrivateKey(this.config.serviceAccount.private_key);
      assertion = await signRs256(
        { alg: "RS256", typ: "JWT" },
        {
          iss: this.config.serviceAccount.client_email,
          scope: FCM_SCOPE,
          aud: GOOGLE_TOKEN_URL,
          iat: now,
          exp: now + 3600,
        },
        await this.key,
      );
    } catch {
      return { kind: "failed", reason: "fcm-key-unusable" };
    }
    let response: Response;
    try {
      response = await this.fetcher(GOOGLE_TOKEN_URL, {
        method: "POST",
        headers: { "content-type": "application/x-www-form-urlencoded" },
        body: new URLSearchParams({
          grant_type: "urn:ietf:params:oauth:grant-type:jwt-bearer",
          assertion,
        }).toString(),
      });
    } catch {
      return { kind: "unavailable", retryAfter: null, reason: "google-oauth-unreachable" };
    }
    if (!response.ok) {
      return response.status >= 500
        ? { kind: "unavailable", retryAfter: retryAfterSeconds(response), reason: `google-oauth-${response.status}` }
        : { kind: "failed", reason: `google-oauth-${response.status}` };
    }
    const body = (await response.json().catch(() => null)) as
      | { access_token?: unknown; expires_in?: unknown }
      | null;
    if (!body || typeof body.access_token !== "string") {
      return { kind: "failed", reason: "google-oauth-malformed" };
    }
    const expiresIn = typeof body.expires_in === "number" ? body.expires_in : 3600;
    this.cached = { token: body.access_token, expiresAt: now + expiresIn };
    return body.access_token;
  }

  async send(request: DeliveryRequest): Promise<DeliveryOutcome> {
    const token = await this.accessToken(request.now);
    if (typeof token !== "string") return token;

    let response: Response;
    try {
      response = await this.fetcher(
        `https://fcm.googleapis.com/v1/projects/${encodeURIComponent(this.config.projectId)}/messages:send`,
        {
          method: "POST",
          headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
          body: JSON.stringify(fcmMessage(request)),
        },
      );
    } catch {
      return { kind: "unavailable", retryAfter: null, reason: "fcm-unreachable" };
    }
    if (response.ok) return { kind: "delivered" };

    const body = (await response.json().catch(() => null)) as FcmErrorBody | null;
    const fcmErrorCode = body?.error?.details?.find((detail) => typeof detail.errorCode === "string")?.errorCode;
    const errorCode = fcmErrorCode ?? body?.error?.status ?? `http-${response.status}`;
    const reason = /^[A-Z_0-9-]{1,64}$/iu.test(errorCode) ? errorCode : `http-${response.status}`;

    // Only answers about this one registration token invalidate a device.
    // SENDER_ID_MISMATCH means the token belongs to another Firebase project:
    // with a wrong FCM_PROJECT_ID every device would answer it, so it is a relay
    // configuration failure, like any authentication or permission error.
    // UNREGISTERED comes only for tokens FCM once issued.
    if (fcmErrorCode === "UNREGISTERED") return { kind: "device-invalid", reason, evidence: "expired" };
    if (fcmErrorCode === "INVALID_ARGUMENT" && /registration token/iu.test(body?.error?.message ?? "")) {
      return { kind: "device-invalid", reason, evidence: "rejected" };
    }
    if (response.status === 401 || response.status === 403) {
      this.cached = null;
      return { kind: "failed", reason };
    }
    if (response.status === 429 || reason === "QUOTA_EXCEEDED") {
      return { kind: "throttled", retryAfter: retryAfterSeconds(response) ?? 60, reason };
    }
    if (response.status >= 500) {
      return { kind: "unavailable", retryAfter: retryAfterSeconds(response), reason };
    }
    return { kind: "failed", reason };
  }
}
