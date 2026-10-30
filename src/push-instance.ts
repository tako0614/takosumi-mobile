/**
 * The instance's pusher API for Takosumi Mobile (takosumi
 * `docs/reference/api.md`「Takosumi Mobile の通知の登録」):
 * `GET/POST /api/v1/account/notification-pushers`, `DELETE .../{id}`.
 */

import {
  normalizeHostUrl,
  sameOriginApiBaseUrl,
  type MobileSession,
} from "@takosjp/mobile-kit";

export type InstanceFetch = (input: string, init?: RequestInit) => Promise<Response>;

/** Why an instance cannot send Web Push (`web_push_unavailable_reason`). */
export type WebPushUnavailableReason = "secret_not_configured" | "relay_not_configured" | "issuer_not_https";

export interface InstancePusher {
  readonly id: string;
  readonly appId: string;
  readonly pushkey: string;
  readonly endpoint: string;
  readonly failing: boolean;
}

export interface InstancePusherConfig {
  readonly pushers: readonly InstancePusher[];
  /** The instance's VAPID public key, or null with `unavailableReason`. */
  readonly webPushPublicKey: string | null;
  readonly unavailableReason: WebPushUnavailableReason | null;
}

export interface PusherRegistration {
  readonly pushkey: string;
  readonly endpoint: string;
  readonly auth: string;
  readonly deviceDisplayName: string;
  readonly lang: string;
}

export const TAKOSUMI_MOBILE_APP_ID = "com.takosumi";
const PUSHERS_PATH = "/account/notification-pushers";
const ID = /^[A-Za-z0-9_-]{1,128}$/u;
const KEY = /^[A-Za-z0-9_-]{86,88}$/u;
const LANG = /^[A-Za-z]{2,8}(?:-[A-Za-z0-9]{1,8}){0,7}$/u;

/** An instance answer other than success, with Takosumi's error code. */
export class InstancePushError extends Error {
  readonly status: number;
  readonly code: string | undefined;
  readonly reason: string | undefined;

  constructor(status: number, code: string | undefined, reason: string | undefined) {
    super(`Takosumi answered ${status}${code ? ` ${code}` : ""}${reason ? ` (${reason})` : ""}.`);
    this.name = "InstancePushError";
    this.status = status;
    this.code = code;
    this.reason = reason;
  }
}

function apiUrl(session: MobileSession, path: string): string {
  const base =
    sameOriginApiBaseUrl(session.apiBaseUrl, session.hostUrl) ?? `${normalizeHostUrl(session.hostUrl)}/api/v1`;
  return `${base.replace(/\/+$/u, "")}${path}`;
}

async function call(
  session: MobileSession,
  fetcher: InstanceFetch,
  path: string,
  init: { readonly method: string; readonly body?: unknown },
): Promise<unknown> {
  const response = await fetcher(apiUrl(session, path), {
    method: init.method,
    headers: {
      accept: "application/json",
      authorization: `${session.tokenType} ${session.accessToken}`,
      ...(init.body === undefined ? {} : { "content-type": "application/json" }),
    },
    ...(init.body === undefined ? {} : { body: JSON.stringify(init.body) }),
  });
  if (response.status === 204) return undefined;
  const text = await response.text().catch(() => "");
  let body: unknown;
  try {
    body = text ? JSON.parse(text) : undefined;
  } catch {
    body = undefined;
  }
  if (!response.ok) {
    const error = record(record(body)?.error);
    const code = typeof error?.code === "string" ? error.code : undefined;
    const details = record(error?.details);
    const reason = typeof details?.reason === "string" ? details.reason : undefined;
    throw new InstancePushError(response.status, code, reason);
  }
  return body;
}

export async function getPusherConfig(session: MobileSession, fetcher: InstanceFetch): Promise<InstancePusherConfig> {
  const body = record(await call(session, fetcher, PUSHERS_PATH, { method: "GET" }));
  if (!body || !Array.isArray(body.pushers)) throw new Error("The pusher list is malformed.");
  const key = body.web_push_public_key;
  if (key !== null && (typeof key !== "string" || !KEY.test(key))) {
    throw new Error("web_push_public_key is malformed.");
  }
  const reason = body.web_push_unavailable_reason;
  const unavailableReason: WebPushUnavailableReason | null =
    reason === "secret_not_configured" || reason === "relay_not_configured" || reason === "issuer_not_https"
      ? reason
      : null;
  return {
    pushers: body.pushers.map(decodePusher).filter((pusher) => pusher !== undefined),
    webPushPublicKey: key,
    unavailableReason: key === null ? (unavailableReason ?? "secret_not_configured") : null,
  };
}

export async function registerPusher(
  session: MobileSession,
  fetcher: InstanceFetch,
  registration: PusherRegistration,
): Promise<InstancePusher> {
  const body = record(
    await call(session, fetcher, PUSHERS_PATH, {
      method: "POST",
      body: {
        kind: "webpush",
        app_id: TAKOSUMI_MOBILE_APP_ID,
        pushkey: registration.pushkey,
        data: { url: registration.endpoint, auth: registration.auth },
        device_display_name: registration.deviceDisplayName.trim().slice(0, 128) || "Takosumi Mobile",
        lang: LANG.test(registration.lang) ? registration.lang : "en",
      },
    }),
  );
  const pusher = decodePusher(body?.pusher);
  if (!pusher) throw new Error("The registered pusher is malformed.");
  return pusher;
}

/** Deletes a pusher; one the instance no longer has counts as deleted. */
export async function deletePusher(session: MobileSession, fetcher: InstanceFetch, pusherId: string): Promise<void> {
  if (!ID.test(pusherId)) return;
  try {
    await call(session, fetcher, `${PUSHERS_PATH}/${encodeURIComponent(pusherId)}`, { method: "DELETE" });
  } catch (error) {
    if (error instanceof InstancePushError && error.status === 404) return;
    throw error;
  }
}

/** The device language as a BCP 47 tag the instance accepts (`ja-JP`). */
export function deviceLanguage(value: string | undefined): string {
  const tag = value?.trim() ?? "";
  return LANG.test(tag) && tag.length <= 35 ? tag : "en";
}

function decodePusher(value: unknown): InstancePusher | undefined {
  const pusher = record(value);
  const data = record(pusher?.data);
  if (!pusher || !data) return undefined;
  const { id, app_id: appId, pushkey } = pusher;
  if (typeof id !== "string" || !ID.test(id)) return undefined;
  if (typeof appId !== "string" || typeof pushkey !== "string" || typeof data.url !== "string") return undefined;
  return { id, appId, pushkey, endpoint: data.url, failing: pusher.failing === true };
}

function record(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}
