import navigationRules from "../src-tauri/plugins/app-screen/navigation-policy-cases.json";
import resumeRules from "../src-tauri/plugins/app-screen/resume-url-cases.json";

/**
 * Navigation rules for app screens (the native webviews that show installed
 * apps). The native plugins enforce the same table at navigation time:
 * `src-tauri/plugins/app-screen/navigation-policy-cases.json` is the shared
 * fixture, checked here by tests and mirrored by `AppScreenPolicy.kt` and
 * `AppScreenPolicy.swift`.
 *
 * - The main frame shows https pages only. `about:blank` / `about:srcdoc`
 *   stay in the screen because `window.open()` popups start there.
 * - A screen shows its app's own origin in the main frame. On the Takosumi
 *   origins (the instance's Accounts and the instance itself, where the
 *   dashboard and its session live) it shows only the sign-in pages an app's
 *   Takosumi sign-in passes through (`ACCOUNTS_SIGN_IN_PATHS`); every other
 *   page there goes to the browser view and is refused in subframes, so the
 *   management screens never run with the app screen's web profile. Any other
 *   https page (an external sign-in such as Google, which forbids embedded
 *   webviews) goes to the browser view (Custom Tabs /
 *   `SFSafariViewController`) instead.
 * - `http:` and other schemes (`tel:`, `mailto:`, ...) leave the app only
 *   after the person confirms, and only from the main frame.
 * - Schemes that would run or read local content (`javascript:`, `data:`,
 *   `file:`, ...), Takosumi Mobile's own `takosumi:` scheme and the hosts the
 *   Tauri shell serves itself from (`tauri.localhost`, `ipc.localhost`, any
 *   `*.localhost`) are refused in every frame, so a page cannot drive the
 *   shell (for example a forged OIDC callback).
 * - URLs with user info (`https://bank.example@evil.example`) are refused in
 *   the main frame.
 */

export type AppScreenNavigationDecision =
  | { readonly kind: "in-app" }
  | { readonly kind: "browser-view"; readonly reason: "other-origin" | "takosumi-page" }
  | {
      readonly kind: "external-confirm";
      readonly reason: "insecure-http" | "other-scheme";
      readonly scheme: string;
    }
  | {
      readonly kind: "block";
      readonly reason:
        | "invalid-url"
        | "user-info"
        | "local-content"
        | "shell-scheme"
        | "shell-host"
        | "takosumi-page"
        | "subframe-scheme";
    };

/** What one app screen may show (see `decideAppScreenNavigation`). */
export interface AppScreenOrigins {
  /** The app's own origin, shown in full. */
  readonly appOrigin: string;
  /**
   * The Takosumi origins (the instance's Accounts and the instance itself):
   * only `ACCOUNTS_SIGN_IN_PATHS` are shown there.
   */
  readonly accountsOrigins: readonly string[];
}

export interface AppScreenNavigationContext {
  readonly mainFrame: boolean;
  /** The screen's origins. Without it only the scheme rules apply (launch URL checks). */
  readonly screen?: AppScreenOrigins;
}

/**
 * The pages an app's Takosumi sign-in passes through on the Takosumi origins,
 * compared exactly on the percent-encoded path: `/oauth/authorize`
 * (authorization and consent), the sign-in screen it redirects to and its
 * callback, and the upstream sign-in (`/oauth/upstream/*`). Takosumi's routes:
 * `contract/identity-oidc.ts`, `accounts/contract` and the dashboard's
 * `/sign-in` routes.
 */
export const ACCOUNTS_SIGN_IN_PATHS: ReadonlySet<string> = new Set(
  navigationRules.accountsSignInPaths,
);

/** Takosumi Mobile's own deep-link scheme; pages must not navigate to it. */
export const SHELL_SCHEME = "takosumi";

/**
 * Hosts the Tauri shell serves its own UI and IPC from. App screens share the
 * device's web profile with the shell, so they never load these hosts, nor
 * any other `*.localhost` name (Tauri's custom protocols use them).
 */
export const SHELL_HOSTS: ReadonlySet<string> = new Set([
  "tauri.localhost",
  "ipc.localhost",
  "asset.localhost",
]);

export function isShellHost(host: string): boolean {
  const name = host.toLowerCase().replace(/\.+$/u, "");
  return SHELL_HOSTS.has(name) || name.endsWith(".localhost");
}

const IN_SCREEN_ABOUT_URLS = new Set(["about:blank", "about:srcdoc"]);

/**
 * Schemes that are never handed to the OS: they execute script, expose local
 * or browser-internal content, or are not navigations at all.
 */
const LOCAL_CONTENT_SCHEMES = new Set([
  "about",
  "blob",
  "chrome",
  "chrome-extension",
  "content",
  "data",
  "file",
  "filesystem",
  "javascript",
  "view-source",
  "ws",
  "wss",
]);

/** Schemes a subframe may load; the webview's own rules apply to them. */
const SUBFRAME_WEB_SCHEMES = new Set([
  "about",
  "blob",
  "data",
  "http",
  "https",
  "javascript",
]);

export function decideAppScreenNavigation(
  rawUrl: string,
  context: AppScreenNavigationContext,
): AppScreenNavigationDecision {
  const url = parseUrl(rawUrl);
  if (!url) return { kind: "block", reason: "invalid-url" };
  const scheme = url.protocol.slice(0, -1).toLowerCase();
  const web = scheme === "http" || scheme === "https";
  if (web && isShellHost(url.hostname)) return { kind: "block", reason: "shell-host" };

  if (!context.mainFrame) {
    if (!SUBFRAME_WEB_SCHEMES.has(scheme)) {
      return { kind: "block", reason: "subframe-scheme" };
    }
    if (web && context.screen && isTakosumiPageOutsideSignIn(url, context.screen)) {
      return { kind: "block", reason: "takosumi-page" };
    }
    return { kind: "in-app" };
  }

  if (scheme === "https") {
    if (url.username || url.password || hasRawUserInfo(rawUrl)) {
      return { kind: "block", reason: "user-info" };
    }
    if (!url.hostname) return { kind: "block", reason: "invalid-url" };
    const screen = context.screen;
    if (screen) {
      if (isTakosumiOrigin(url.origin, screen)) {
        return ACCOUNTS_SIGN_IN_PATHS.has(url.pathname)
          ? { kind: "in-app" }
          : { kind: "browser-view", reason: "takosumi-page" };
      }
      if (url.origin !== screen.appOrigin) {
        return { kind: "browser-view", reason: "other-origin" };
      }
    }
    return { kind: "in-app" };
  }
  if (scheme === "about" && IN_SCREEN_ABOUT_URLS.has(rawUrl.trim().toLowerCase())) {
    return { kind: "in-app" };
  }
  if (scheme === SHELL_SCHEME) return { kind: "block", reason: "shell-scheme" };
  if (LOCAL_CONTENT_SCHEMES.has(scheme)) {
    return { kind: "block", reason: "local-content" };
  }
  if (scheme === "intent") {
    // Android `intent:` URIs name their real scheme in the fragment; judge that.
    const target = intentTargetScheme(rawUrl);
    if (target === SHELL_SCHEME) return { kind: "block", reason: "shell-scheme" };
    if (target !== undefined && LOCAL_CONTENT_SCHEMES.has(target)) {
      return { kind: "block", reason: "local-content" };
    }
  }
  if (scheme === "http") {
    if (url.username || url.password || hasRawUserInfo(rawUrl)) {
      return { kind: "block", reason: "user-info" };
    }
    return { kind: "external-confirm", reason: "insecure-http", scheme };
  }
  return { kind: "external-confirm", reason: "other-scheme", scheme };
}

/** A page of a Takosumi origin that is not one of the sign-in pages. */
function isTakosumiPageOutsideSignIn(url: URL, screen: AppScreenOrigins): boolean {
  return isTakosumiOrigin(url.origin, screen) && !ACCOUNTS_SIGN_IN_PATHS.has(url.pathname);
}

/**
 * A Takosumi origin, also written with a trailing dot in the host
 * (`https://app.takosumi.com./`), which reaches the same server.
 */
function isTakosumiOrigin(origin: string, screen: AppScreenOrigins): boolean {
  const name = originWithoutTrailingDot(origin);
  return screen.accountsOrigins.some((candidate) => originWithoutTrailingDot(candidate) === name);
}

/** `https://host.:443` → `https://host` for comparison. */
export function originWithoutTrailingDot(origin: string): string {
  return origin.replace(/\.+(?=(?::\d+)?$)/u, "");
}

/**
 * An `@` in the authority, even when the user info parses as empty
 * (`https://:@host/`); Android's and Apple's parsers see user info there.
 */
function hasRawUserInfo(rawUrl: string): boolean {
  return /^[a-z][a-z0-9+.-]*:\/\/[^/?#\\]*@/iu.test(rawUrl.trim());
}

/**
 * The `scheme=` of an `intent://...#Intent;scheme=x;end` URI, lowercased.
 * Android turns the URI into a VIEW intent for `x://...`.
 */
export function intentTargetScheme(rawUrl: string): string | undefined {
  // Like Intent.parseUri: the parameters follow the last '#'.
  const hash = rawUrl.lastIndexOf("#");
  if (hash < 0 || !rawUrl.startsWith("Intent;", hash + 1)) return undefined;
  for (const part of rawUrl.slice(hash + "#Intent;".length).split(";")) {
    if (part.toLowerCase().startsWith("scheme=")) {
      return part.slice("scheme=".length).trim().toLowerCase();
    }
  }
  // Without scheme=, `intent:x://...` opens the data part's own scheme.
  const data = rawUrl.slice(rawUrl.indexOf(":") + 1, hash);
  const scheme = /^([a-z][a-z0-9+.-]*):/iu.exec(data)?.[1];
  return scheme?.toLowerCase();
}

/** True when the URL may be opened as the first page of an app screen. */
export function isLaunchableAppScreenUrl(rawUrl: string): boolean {
  const url = parseUrl(rawUrl);
  return (
    url?.protocol === "https:" &&
    decideAppScreenNavigation(rawUrl, { mainFrame: true }).kind === "in-app"
  );
}

/**
 * The origin a screen compares navigations against (`https://host[:port]`,
 * lowercase, default port dropped). `undefined` for opaque origins.
 */
export function screenOrigin(rawUrl: string): string | undefined {
  const url = parseUrl(rawUrl);
  if (!url || url.origin === "null") return undefined;
  return url.origin;
}

/** The https origin of a URL without user info, else `undefined`. */
export function httpsOrigin(raw: string | undefined): string | undefined {
  if (!raw) return undefined;
  const url = parseUrl(raw);
  return url?.protocol === "https:" && !url.username && !url.password ? url.origin : undefined;
}

/**
 * The Takosumi origins of the current instance: its Accounts (the OIDC
 * issuer) and the instance itself (dashboard and API, often the same origin).
 * App screens show only the sign-in pages there.
 */
export function takosumiOrigins(input: {
  readonly hostUrl?: string;
  readonly oidcIssuer?: string;
}): readonly string[] {
  const origins: string[] = [];
  for (const origin of [httpsOrigin(input.oidcIssuer), httpsOrigin(input.hostUrl)]) {
    if (origin && !origins.includes(origin)) origins.push(origin);
  }
  return origins;
}

/** The origins one app screen judges navigations by. */
export function appScreenOrigins(input: {
  readonly appUrl: string;
  readonly accountsOrigins: readonly string[];
}): AppScreenOrigins | undefined {
  const appOrigin = screenOrigin(input.appUrl);
  return appOrigin ? { appOrigin, accountsOrigins: input.accountsOrigins } : undefined;
}

/**
 * True when the app screen of `appUrl` shows `appUrl` itself: false for a
 * Takosumi page outside sign-in (the dashboard, `/api/v1`, account pages),
 * which opens in the browser view instead, never in an app screen.
 */
export function opensInAppScreen(appUrl: string, accountsOrigins: readonly string[]): boolean {
  const screen = appScreenOrigins({ appUrl, accountsOrigins });
  return (
    screen !== undefined &&
    isLaunchableAppScreenUrl(appUrl) &&
    decideAppScreenNavigation(appUrl, { mainFrame: true, screen }).kind === "in-app"
  );
}

/** Longest URL a screen remembers as the place to return to. */
export const RESUME_URL_MAX_LENGTH = 4_096;

/**
 * Query parameter names (lowercase letters and digits only) that carry a
 * credential. Takosumi's own list (`hasCredentialQueryParams` in
 * `contract/interface-display.ts`) plus the OAuth / SAML callback values.
 * Shared with the native plugins through `resume-url-cases.json`.
 */
export const CREDENTIAL_QUERY_NAMES: ReadonlySet<string> = new Set(
  resumeRules.credentialQueryNames,
);

/**
 * True when a screen may remember `rawUrl` as the place to reopen at: an
 * https page of the app's own origin with no user info, no fragment (where
 * implicit-flow tokens travel) and no credential-named query parameter.
 * Takosumi's `safeLaunchUrl` rules, plus the OAuth callback values.
 */
export function isRememberableAppScreenUrl(
  rawUrl: string,
  appOrigin: string,
): boolean {
  if (rawUrl.length > RESUME_URL_MAX_LENGTH || rawUrl.includes("#")) return false;
  const url = parseUrl(rawUrl);
  if (!url || url.protocol !== "https:" || url.username || url.password) {
    return false;
  }
  if (url.origin !== appOrigin || !isLaunchableAppScreenUrl(rawUrl)) return false;
  if (/\s/u.test(rawUrl)) return false;
  const query = rawUrl.includes("?") ? rawUrl.slice(rawUrl.indexOf("?") + 1) : "";
  for (const pair of query.split("&")) {
    if (!pair) continue;
    const parsed = queryPair(pair);
    if (!parsed || isCredentialPair(parsed)) return false;
  }
  return true;
}

/**
 * `rawUrl` as it may be shared or handed to the browser: query parameters
 * with a credential name and a value, and a fragment that holds parameters
 * (where implicit-flow tokens travel), are removed; everything else is kept
 * as written. A pair whose name cannot be decoded is removed too.
 * `resume-url-cases.json` `shareCases` is the shared table.
 */
export function shareableAppScreenUrl(rawUrl: string): string {
  const hash = rawUrl.indexOf("#");
  const beforeHash = hash < 0 ? rawUrl : rawUrl.slice(0, hash);
  const fragment = hash < 0 ? undefined : rawUrl.slice(hash + 1);
  const question = beforeHash.indexOf("?");
  const base = question < 0 ? beforeHash : beforeHash.slice(0, question);
  const kept = (question < 0 ? "" : beforeHash.slice(question + 1))
    .split("&")
    .filter((pair) => {
      if (!pair) return false;
      const parsed = queryPair(pair);
      return parsed !== undefined && !isCredentialPair(parsed);
    });
  return (
    base +
    (kept.length ? `?${kept.join("&")}` : "") +
    (fragment !== undefined && !fragment.includes("=") ? `#${fragment}` : "")
  );
}

/** As URLSearchParams decodes a pair, but `undefined` when it cannot. */
function queryPair(pair: string): { readonly name: string; readonly value: string } | undefined {
  const equals = pair.indexOf("=");
  const decode = (part: string) => {
    try {
      return decodeURIComponent(part.replaceAll("+", " "));
    } catch {
      return undefined;
    }
  };
  const name = decode(equals < 0 ? pair : pair.slice(0, equals));
  const value = decode(equals < 0 ? "" : pair.slice(equals + 1));
  return name === undefined || value === undefined ? undefined : { name, value };
}

function isCredentialPair(pair: { readonly name: string; readonly value: string }): boolean {
  const normalized = pair.name.toLowerCase().replace(/[^a-z0-9]/gu, "");
  return pair.value.trim() !== "" && CREDENTIAL_QUERY_NAMES.has(normalized);
}

export type NormalizedAppUrl =
  | { readonly ok: true; readonly url: string; readonly host: string }
  | { readonly ok: false; readonly error: string };

/**
 * Normalize a URL typed by the person ("example.com" → "https://example.com/").
 * Only https URLs without user info are accepted; the fragment is dropped.
 */
export function normalizeUserAppUrl(input: string): NormalizedAppUrl {
  const raw = input.trim();
  if (!raw) return { ok: false, error: "URLを入力してください。" };
  if (/\s/u.test(raw)) {
    return { ok: false, error: "URLに空白は使えません。" };
  }
  const hasScheme = /^[a-z][a-z0-9+.-]*:/iu.test(raw);
  const looksLikeHostPort = /^[^/?#:@]+:\d+(?:[/?#]|$)/u.test(raw);
  const candidate = hasScheme && !looksLikeHostPort ? raw : `https://${raw}`;
  const url = parseUrl(candidate);
  if (!url || !url.hostname) {
    return { ok: false, error: "URLの形が正しくありません。" };
  }
  if (url.protocol !== "https:") {
    return { ok: false, error: "https のURLだけ追加できます。" };
  }
  if (url.username || url.password) {
    return { ok: false, error: "ユーザー名やパスワードを含むURLは追加できません。" };
  }
  if (!url.hostname.includes(".") && url.hostname !== "localhost") {
    return { ok: false, error: "URLの形が正しくありません。" };
  }
  url.hash = "";
  if (!isLaunchableAppScreenUrl(url.toString())) {
    return { ok: false, error: "このURLは追加できません。" };
  }
  return { ok: true, url: url.toString(), host: url.host };
}

/**
 * The host shown in the app screen's top bar. It is the ASCII (punycode)
 * host, so look-alike Unicode names are visible as such.
 */
export function displayHost(rawUrl: string): string {
  const url = parseUrl(rawUrl);
  return url?.host ?? "";
}

export function sameOrigin(a: string, b: string): boolean {
  const left = parseUrl(a);
  const right = parseUrl(b);
  return Boolean(
    left && right && left.origin !== "null" && left.origin === right.origin,
  );
}

function parseUrl(raw: string): URL | undefined {
  try {
    return new URL(raw.trim());
  } catch {
    return undefined;
  }
}
