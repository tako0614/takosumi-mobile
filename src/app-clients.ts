import {
  INTERFACE_CLIENT_BLOCKED_SCHEMES,
  INTERFACE_CLIENT_IDENTIFIER_MAX_LENGTH,
  INTERFACE_CLIENT_LINK_MAX_LENGTH,
  INTERFACE_CLIENT_NAME_MAX_LENGTH,
} from "../../takosumi/contract/interface-clients.ts";
import { SHELL_SCHEME } from "./url-policy.ts";

/**
 * The native phone apps a screen declares (`screens[].clients` of
 * `GET /api/v1/capsules/{capsuleId}/app`, TASK-0119 step 2). The instance
 * already checked the declaration against its trusted native clients
 * (`parseInterfaceClients` in takosumi `contract/interface-clients.ts`) and
 * filled in the links; Takosumi Mobile re-checks the shape it relies on and
 * drops what does not hold, never repairing a value.
 *
 * Only a `verified` client may open without asking. An unverified client is
 * shown with a neutral label and its real destination (`target`), derived
 * here from the very links that would be opened, and opens only after the
 * person confirms. It is never labelled with the screen's title.
 */

export type ClientPlatform = "ios" | "android";

/** Where a client's links really go. */
export interface ScreenClientTarget {
  /** Android: the only app that can receive the links. */
  readonly package?: string;
  /** The custom scheme of `open`. On iOS any installed app may claim it. */
  readonly scheme?: string;
  /** The host of the universal link / App Link. */
  readonly host?: string;
}

interface ScreenClientBase {
  readonly verified: boolean;
  /** Verified: the trusted list's name. Unverified: the checked declared name. */
  readonly name?: string;
  readonly target: ScreenClientTarget;
  /** Custom-scheme link, filled in by the instance. */
  readonly open?: string;
  /** Where to install the app (an App Store, Google Play or F-Droid page). */
  readonly storeUrl?: string;
}

export interface ScreenIosClient extends ScreenClientBase {
  readonly platform: "ios";
  readonly universalLink?: string;
}

export interface ScreenAndroidClient extends ScreenClientBase {
  readonly platform: "android";
  readonly package: string;
  readonly appLink?: string;
}

export type ScreenClient = ScreenIosClient | ScreenAndroidClient;

export interface ScreenClients {
  /** `"native"` only when some platform's client is verified. */
  readonly preferred: "native" | "web";
  readonly ios?: ScreenIosClient;
  readonly android?: ScreenAndroidClient;
}

/** Shown for an unverified client that has no checked name. */
export const UNVERIFIED_CLIENT_LABEL = "未確認のアプリ";

/** Takosumi Mobile's own identity; a declaration can never open it. */
const OWN_APP_IDS = new Set(["com.takosumi"]);
const SCHEME_PATTERN = /^[a-z][a-z0-9+.-]{1,31}$/u;
const REVERSE_DNS_PATTERN = /^[A-Za-z][A-Za-z0-9_-]*(?:\.[A-Za-z][A-Za-z0-9_-]*)+$/u;
const ANDROID_PACKAGE_PATTERN = /^[A-Za-z][A-Za-z0-9_]*(?:\.[A-Za-z][A-Za-z0-9_]*)+$/u;
const APP_STORE_ID_PATTERN = /^[0-9]{1,12}$/u;
const IOS_STORE_HOSTS = new Set(["apps.apple.com", "testflight.apple.com"]);
const ANDROID_STORE_HOSTS = new Set(["play.google.com", "f-droid.org"]);

export function decodeScreenClients(value: unknown): ScreenClients | undefined {
  if (!isRecord(value)) return undefined;
  const ios = decodeIosClient(value.ios);
  const android = decodeAndroidClient(value.android);
  if (!ios && !android) return undefined;
  const anyVerified = ios?.verified === true || android?.verified === true;
  return {
    preferred: anyVerified && value.preferred === "native" ? "native" : "web",
    ...(ios ? { ios } : {}),
    ...(android ? { android } : {}),
  };
}

function decodeIosClient(value: unknown): ScreenIosClient | undefined {
  if (!isRecord(value) || typeof value.verified !== "boolean") return undefined;
  const bundleId = identifier(value.bundleId, REVERSE_DNS_PATTERN);
  if (value.bundleId !== undefined && bundleId === undefined) return undefined;
  const open = customSchemeLink(value.open);
  const universalLink = httpsLink(value.universalLink);
  if (!open && !universalLink) return undefined;
  const name = clientName(value.name);
  // A verified client always carries the trusted list's name.
  const verified = value.verified && name !== undefined;
  const storeUrl =
    storeLink(value.storeUrl, IOS_STORE_HOSTS) ?? appStoreLink(value.appStoreId);
  return {
    platform: "ios",
    verified,
    ...(name === undefined ? {} : { name }),
    target: {
      ...(open ? { scheme: schemeOf(open) } : {}),
      ...(universalLink ? { host: new URL(universalLink).host } : {}),
    },
    ...(open ? { open } : {}),
    ...(universalLink ? { universalLink } : {}),
    ...(storeUrl ? { storeUrl } : {}),
  };
}

function decodeAndroidClient(value: unknown): ScreenAndroidClient | undefined {
  if (!isRecord(value) || typeof value.verified !== "boolean") return undefined;
  const packageName = identifier(value.package, ANDROID_PACKAGE_PATTERN);
  if (!packageName) return undefined;
  const open = customSchemeLink(value.open);
  const appLink = httpsLink(value.appLink);
  if (!open && !appLink) return undefined;
  const name = clientName(value.name);
  const verified = value.verified && name !== undefined;
  const storeUrl =
    storeLink(value.storeUrl, ANDROID_STORE_HOSTS) ??
    `https://play.google.com/store/apps/details?id=${encodeURIComponent(packageName)}`;
  return {
    platform: "android",
    verified,
    ...(name === undefined ? {} : { name }),
    package: packageName,
    target: {
      package: packageName,
      ...(open ? { scheme: schemeOf(open) } : {}),
      ...(appLink ? { host: new URL(appLink).host } : {}),
    },
    ...(open ? { open } : {}),
    ...(appLink ? { appLink } : {}),
    storeUrl,
  };
}

/** The client for the phone Takosumi Mobile runs on, if one is declared. */
export function clientForPlatform(
  clients: ScreenClients | undefined,
  platform: string,
): ScreenClient | undefined {
  if (platform === "ios") return clients?.ios;
  if (platform === "android") return clients?.android;
  return undefined;
}

/**
 * The name to show: the trusted name when verified, the checked declared
 * name or the neutral label otherwise. Never the screen's title.
 */
export function clientLabel(client: ScreenClient): string {
  return client.name ?? UNVERIFIED_CLIENT_LABEL;
}

/** The real destination, as shown before opening an unverified client. */
export function clientTargetLabel(client: ScreenClient): string {
  const parts: string[] = [];
  if (client.target.package) parts.push(client.target.package);
  if (client.target.host) parts.push(client.target.host);
  if (client.target.scheme) parts.push(`${client.target.scheme}://`);
  return parts.join(" · ");
}

/** The host of the install link (App Store, Google Play, F-Droid). */
export function clientStoreHost(client: ScreenClient): string | undefined {
  return client.storeUrl ? new URL(client.storeUrl).host : undefined;
}

/** What `open_client` needs for this client; nothing else is sent. */
export interface OpenClientRequest {
  readonly platform: ClientPlatform;
  readonly package?: string;
  readonly appLink?: string;
  readonly universalLink?: string;
  readonly open?: string;
}

export function openClientRequest(client: ScreenClient): OpenClientRequest {
  if (client.platform === "android") {
    return {
      platform: "android",
      package: client.package,
      ...(client.appLink ? { appLink: client.appLink } : {}),
      ...(client.open ? { open: client.open } : {}),
    };
  }
  return {
    platform: "ios",
    ...(client.universalLink ? { universalLink: client.universalLink } : {}),
    ...(client.open ? { open: client.open } : {}),
  };
}

function clientName(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const name = value.trim();
  if (!name || [...name].length > INTERFACE_CLIENT_NAME_MAX_LENGTH) return undefined;
  // Control, format (bidi overrides, zero-width) and separator characters.
  if (/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u.test(name)) return undefined;
  return name;
}

function identifier(value: unknown, pattern: RegExp): string | undefined {
  if (
    typeof value !== "string" ||
    value.length > INTERFACE_CLIENT_IDENTIFIER_MAX_LENGTH ||
    !pattern.test(value) ||
    OWN_APP_IDS.has(value.toLowerCase())
  ) {
    return undefined;
  }
  return value;
}

function customSchemeLink(value: unknown): string | undefined {
  if (typeof value !== "string" || value.length > INTERFACE_CLIENT_LINK_MAX_LENGTH) {
    return undefined;
  }
  if (/[\s\p{Cc}]/u.test(value)) return undefined;
  // The instance writes the scheme lowercase; anything else is not its link.
  const scheme = schemeOf(value);
  if (
    !scheme ||
    !SCHEME_PATTERN.test(scheme) ||
    scheme === SHELL_SCHEME ||
    INTERFACE_CLIENT_BLOCKED_SCHEMES.has(scheme)
  ) {
    return undefined;
  }
  return value;
}

function httpsLink(value: unknown): string | undefined {
  if (typeof value !== "string" || value.length > INTERFACE_CLIENT_LINK_MAX_LENGTH) {
    return undefined;
  }
  try {
    const url = new URL(value);
    if (url.protocol !== "https:" || url.username || url.password || !url.hostname) {
      return undefined;
    }
    return value;
  } catch {
    return undefined;
  }
}

function storeLink(value: unknown, hosts: ReadonlySet<string>): string | undefined {
  const link = httpsLink(value);
  if (!link) return undefined;
  const url = new URL(link);
  return url.port === "" && hosts.has(url.hostname) ? link : undefined;
}

function appStoreLink(value: unknown): string | undefined {
  return typeof value === "string" && APP_STORE_ID_PATTERN.test(value)
    ? `https://apps.apple.com/app/id${value}`
    : undefined;
}

function schemeOf(link: string): string | undefined {
  return /^([a-z][a-z0-9+.-]*):/u.exec(link)?.[1];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
