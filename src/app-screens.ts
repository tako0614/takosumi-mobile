import type { WireDecoder } from "@takosjp/mobile-kit";
import {
  displayHost,
  isLaunchableAppScreenUrl,
  normalizeUserAppUrl,
  sameOrigin,
  SHELL_SCHEME,
} from "./url-policy.ts";

/**
 * Home launcher model: the installed apps' launchable screens (from
 * `GET /api/v1/capsules/{capsuleId}/app`), plus https URLs the person added
 * on this device. Each entry opens in a native app screen
 * (`src-tauri/plugins/app-screen`).
 */

export const CAPSULE_APP_VIEW_KIND = "takosumi.capsule-app@v1";
const GLYPH_MAX_CODE_POINTS = 16;
const ICON_URL_MAX_LENGTH = 2_048;
const LABEL_MAX_LENGTH = 64;
const KEY_MAX_LENGTH = 512;

export type AppScreenIcon =
  | { readonly kind: "image"; readonly url: string }
  | { readonly kind: "glyph"; readonly glyph: string };

/** One launchable screen (a `ui-surface` Interface the caller may open). */
export interface CapsuleAppScreen {
  readonly interfaceId: string;
  readonly name: string;
  readonly title?: string;
  readonly icon?: AppScreenIcon;
  readonly sortOrder?: number;
  readonly url: string;
}

export interface CapsuleAppScreens {
  readonly capsuleId: string;
  readonly screens: readonly CapsuleAppScreen[];
  /** False when the host reported the screens section as incomplete. */
  readonly complete: boolean;
}

/**
 * Decoder for the screens part of the installed-app view. Display metadata
 * degrades to absence; a screen without an https launch URL is left out,
 * because app screens open https pages only.
 */
export const CAPSULE_APP_SCREENS_DECODER: WireDecoder<CapsuleAppScreens> = {
  document: "Takosumi installed-app view",
  decode(value) {
    const body = requireRecord(value, "document");
    const app = requireRecord(body.app, "app");
    if (app.kind !== undefined && app.kind !== CAPSULE_APP_VIEW_KIND) {
      throw new Error(`app.kind must be ${CAPSULE_APP_VIEW_KIND}`);
    }
    const capsuleId = requireString(app.capsuleId, "app.capsuleId");
    const screens = Array.isArray(app.screens) ? app.screens : [];
    const incomplete = Array.isArray(app.incomplete) ? app.incomplete : [];
    return {
      capsuleId,
      screens: screens.flatMap((item) => {
        const screen = decodeScreen(item);
        return screen ? [screen] : [];
      }),
      complete: !incomplete.includes("screens") && Array.isArray(app.screens),
    };
  },
};

function decodeScreen(value: unknown): CapsuleAppScreen | undefined {
  if (!isRecord(value)) return undefined;
  const interfaceId = nonEmptyString(value.interfaceId);
  const name = nonEmptyString(value.name);
  const url = nonEmptyString(value.url);
  if (!interfaceId || !name || !url || !isLaunchableAppScreenUrl(url)) {
    return undefined;
  }
  const title = nonEmptyString(value.title);
  const icon = decodeIcon(value.icon);
  const sortOrder =
    typeof value.sortOrder === "number" && Number.isFinite(value.sortOrder)
      ? value.sortOrder
      : undefined;
  return {
    interfaceId,
    name,
    ...(title === undefined ? {} : { title }),
    ...(icon === undefined ? {} : { icon }),
    ...(sortOrder === undefined ? {} : { sortOrder }),
    url,
  };
}

/** Accepts the contract's two icon forms; anything else is dropped. */
export function decodeIcon(value: unknown): AppScreenIcon | undefined {
  if (!isRecord(value)) return undefined;
  if (value.kind === "glyph") {
    const glyph = nonEmptyString(value.glyph);
    return glyph && [...glyph].length <= GLYPH_MAX_CODE_POINTS
      ? { kind: "glyph", glyph }
      : undefined;
  }
  if (value.kind === "image") {
    const url = safeIconUrl(value.url);
    return url ? { kind: "image", url } : undefined;
  }
  return undefined;
}

/** An icon image must be a credential-free absolute https URL. */
export function safeIconUrl(value: unknown): string | undefined {
  const raw = nonEmptyString(value);
  if (!raw || raw.length > ICON_URL_MAX_LENGTH) return undefined;
  try {
    const url = new URL(raw);
    if (url.protocol !== "https:" || url.username || url.password) {
      return undefined;
    }
    return url.toString();
  } catch {
    return undefined;
  }
}

/** An installed app as the launcher needs it (new app list or legacy Capsule). */
export interface LauncherApp {
  readonly capsuleId: string;
  readonly label: string;
  readonly iconUrl?: string;
  readonly state: string;
}

/** A URL the person added; stored only on this device. */
export interface CustomAppEntry {
  readonly id: string;
  readonly url: string;
  readonly title: string;
  readonly addedAt: string;
}

export interface AppLauncherEntry {
  /** Stable identity of the screen: its webview, Android task and shortcut. */
  readonly key: string;
  readonly source: "takosumi" | "custom";
  readonly capsuleId?: string;
  readonly interfaceId?: string;
  readonly customId?: string;
  readonly label: string;
  readonly url: string;
  readonly host: string;
  readonly icon?: AppScreenIcon;
  /** Letter shown when there is no icon or it cannot be loaded. */
  readonly fallbackGlyph: string;
}

/** App states whose screens are worth asking for. */
export function appMayHaveScreens(state: string): boolean {
  return ![
    "installing",
    "uninstalling",
    "uninstalled",
    "destroyed",
    "pending",
  ].includes(state);
}

export function takosumiScreenKey(capsuleId: string, interfaceId: string): string {
  return boundedKey(`app:${capsuleId}:${interfaceId}`);
}

export function customScreenKey(id: string): string {
  return boundedKey(`custom:${id}`);
}

/** Key for a deep-linked URL that matches no known entry. */
export function originScreenKey(url: string): string {
  return boundedKey(`origin:${new URL(url).origin}`);
}

export function buildLauncherEntries(input: {
  readonly apps: readonly LauncherApp[];
  readonly screens: ReadonlyMap<string, CapsuleAppScreens>;
  readonly custom: readonly CustomAppEntry[];
}): readonly AppLauncherEntry[] {
  const entries: AppLauncherEntry[] = [];
  const seenKeys = new Set<string>();
  const push = (entry: AppLauncherEntry) => {
    if (seenKeys.has(entry.key)) return;
    seenKeys.add(entry.key);
    entries.push(entry);
  };

  for (const app of input.apps) {
    const view = input.screens.get(app.capsuleId);
    if (!view || view.capsuleId !== app.capsuleId) continue;
    const appIcon = safeIconUrl(app.iconUrl);
    const single = view.screens.length === 1;
    for (const screen of orderScreens(view.screens)) {
      const label = clampLabel(
        single ? app.label : screen.title ?? `${app.label} ${screen.name}`,
      );
      const icon =
        screen.icon ??
        (appIcon === undefined ? undefined : ({ kind: "image", url: appIcon } as const));
      push({
        key: takosumiScreenKey(app.capsuleId, screen.interfaceId),
        source: "takosumi",
        capsuleId: app.capsuleId,
        interfaceId: screen.interfaceId,
        label,
        url: screen.url,
        host: displayHost(screen.url),
        ...(icon === undefined ? {} : { icon }),
        fallbackGlyph: fallbackGlyph(label),
      });
    }
  }

  for (const custom of input.custom) {
    if (!isLaunchableAppScreenUrl(custom.url)) continue;
    const label = clampLabel(custom.title || displayHost(custom.url));
    push({
      key: customScreenKey(custom.id),
      source: "custom",
      customId: custom.id,
      label,
      url: custom.url,
      host: displayHost(custom.url),
      fallbackGlyph: fallbackGlyph(label),
    });
  }
  return entries;
}

/**
 * Reuse the previous object for entries that did not change, so a home
 * refresh (for example after approving a Run) does not remount every tile.
 */
export function stabilizeLauncherEntries(
  previous: readonly AppLauncherEntry[],
  next: readonly AppLauncherEntry[],
): readonly AppLauncherEntry[] {
  const byKey = new Map(previous.map((entry) => [entry.key, entry]));
  return next.map((entry) => {
    const earlier = byKey.get(entry.key);
    return earlier && JSON.stringify(earlier) === JSON.stringify(entry)
      ? earlier
      : entry;
  });
}

/** Server order is display order; `sortOrder` only breaks ties it leaves. */
function orderScreens(
  screens: readonly CapsuleAppScreen[],
): readonly CapsuleAppScreen[] {
  return screens
    .map((screen, index) => ({ screen, index }))
    .sort((a, b) => {
      const left = a.screen.sortOrder;
      const right = b.screen.sortOrder;
      if (left !== undefined && right !== undefined && left !== right) {
        return left - right;
      }
      return a.index - b.index;
    })
    .map(({ screen }) => screen);
}

export function fallbackGlyph(label: string): string {
  const first = [...label.trim()][0];
  return first ? first.toUpperCase() : "?";
}

function clampLabel(label: string): string {
  const trimmed = label.trim();
  const chars = [...trimmed];
  return chars.length > LABEL_MAX_LENGTH
    ? `${chars.slice(0, LABEL_MAX_LENGTH - 1).join("")}…`
    : trimmed;
}

function boundedKey(key: string): string {
  if (key.length > KEY_MAX_LENGTH) {
    throw new Error("app screen key is too long");
  }
  return key;
}

// --- Deep links -----------------------------------------------------------

/** `takosumi://app?url=<https URL>` opens that URL in an app screen. */
export const APP_SCREEN_DEEP_LINK_HOST = "app";

export function appScreenDeepLink(url: string): string {
  if (!isLaunchableAppScreenUrl(url)) {
    throw new Error("Only https URLs can be opened in an app screen.");
  }
  return `${SHELL_SCHEME}://${APP_SCREEN_DEEP_LINK_HOST}?url=${encodeURIComponent(url)}`;
}

export type ParsedAppScreenDeepLink =
  | { readonly ok: true; readonly url: string }
  | { readonly ok: false; readonly error: string };

/**
 * Returns `undefined` when the payload is not an app-screen deep link, so the
 * caller can hand it to the shared mobile shell (OIDC callback, host routes).
 * Only the URL is read: titles, icons and screen keys come from the known
 * entries, never from the link, so a link cannot dress a page as another app.
 */
export function parseAppScreenDeepLink(
  payload: string,
): ParsedAppScreenDeepLink | undefined {
  let link: URL;
  try {
    link = new URL(payload.trim());
  } catch {
    return undefined;
  }
  if (link.protocol !== `${SHELL_SCHEME}:`) return undefined;
  if (link.hostname !== APP_SCREEN_DEEP_LINK_HOST) return undefined;
  const target = link.searchParams.get("url");
  if (!target) return { ok: false, error: "開くURLがありません。" };
  const normalized = normalizeUserAppUrl(target);
  if (!normalized.ok || !isLaunchableAppScreenUrl(target)) {
    return { ok: false, error: "https のURLだけ開けます。" };
  }
  return { ok: true, url: new URL(target).toString() };
}

export interface AppScreenTarget {
  readonly key: string;
  readonly label: string;
  readonly url: string;
  readonly icon?: AppScreenIcon;
  readonly fallbackGlyph: string;
}

/**
 * A deep-linked URL opens in the screen of the known entry with the same
 * origin (exact URL first), so it reuses that app's webview and task.
 * Otherwise it gets a screen of its own, named by its host.
 */
export function resolveAppScreenTarget(
  url: string,
  entries: readonly AppLauncherEntry[],
): AppScreenTarget {
  const exact = entries.find((entry) => entry.url === url);
  const match = exact ?? entries.find((entry) => sameOrigin(entry.url, url));
  if (match) {
    return {
      key: match.key,
      label: match.label,
      url,
      ...(match.icon === undefined ? {} : { icon: match.icon }),
      fallbackGlyph: match.fallbackGlyph,
    };
  }
  const host = displayHost(url);
  return {
    key: originScreenKey(url),
    label: host,
    url,
    fallbackGlyph: fallbackGlyph(host),
  };
}

// --- Native open request ----------------------------------------------------

/**
 * Every app screen shares one cookie/storage profile, so signing in to
 * Takosumi once on the web carries over to every app that signs in with
 * Takosumi OIDC. The native plugin refuses any other value.
 */
export const APP_SCREEN_PROFILE = "shared" as const;

export interface AppScreenOpenRequest {
  readonly key: string;
  readonly url: string;
  readonly title: string;
  /** Base64 PNG (no data: prefix) for the Android task and shortcuts. */
  readonly iconPng?: string;
  readonly profile: typeof APP_SCREEN_PROFILE;
}

export function appScreenOpenRequest(
  target: Pick<AppScreenTarget, "key" | "label" | "url">,
  iconPng?: string,
): AppScreenOpenRequest {
  if (!isLaunchableAppScreenUrl(target.url)) {
    throw new Error("Only https URLs can be opened in an app screen.");
  }
  return {
    key: boundedKey(target.key),
    url: target.url,
    title: clampLabel(target.label) || displayHost(target.url),
    ...(iconPng ? { iconPng } : {}),
    profile: APP_SCREEN_PROFILE,
  };
}

// --- Unread counts (step 4) -------------------------------------------------

/**
 * Unread counts per launcher entry. Push delivery (TASK-0119 U4 step 4) fills
 * this from each app's Declarative Web Push `app_badge`; until then it is
 * empty and the launcher shows no badges.
 */
export type AppUnreadCounts = ReadonlyMap<string, number>;

export function unreadBadgeLabel(
  counts: AppUnreadCounts,
  key: string,
): string | undefined {
  const count = counts.get(key);
  if (count === undefined || !Number.isFinite(count) || count <= 0) {
    return undefined;
  }
  return count > 99 ? "99+" : String(Math.floor(count));
}

export function totalUnread(counts: AppUnreadCounts): number {
  let total = 0;
  for (const count of counts.values()) {
    if (Number.isFinite(count) && count > 0) total += Math.floor(count);
  }
  return total;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function requireRecord(value: unknown, label: string): Record<string, unknown> {
  if (!isRecord(value)) throw new Error(`${label} must be an object`);
  return value;
}

function requireString(value: unknown, label: string): string {
  const text = nonEmptyString(value);
  if (text === undefined) throw new Error(`${label} must be a non-empty string`);
  return text;
}

function nonEmptyString(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const trimmed = value.trim();
  return trimmed ? trimmed : undefined;
}
