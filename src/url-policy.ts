/**
 * Navigation rules for app screens (the native webviews that show installed
 * apps). The native plugins enforce the same table at navigation time:
 * `src-tauri/plugins/app-screen/navigation-policy-cases.json` is the shared
 * fixture, checked here by tests and mirrored by `AppScreenPolicy.kt` and
 * `AppScreenPolicy.swift`.
 *
 * - The main frame shows https pages only. `about:blank` / `about:srcdoc`
 *   stay in the screen because `window.open()` popups start there.
 * - `http:` and other schemes (`tel:`, `mailto:`, ...) leave the app only
 *   after the person confirms, and only from the main frame.
 * - Schemes that would run or read local content (`javascript:`, `data:`,
 *   `file:`, ...) and Takosumi Mobile's own `takosumi:` scheme are refused,
 *   so a page cannot drive the shell (for example a forged OIDC callback).
 * - URLs with user info (`https://bank.example@evil.example`) are refused in
 *   the main frame.
 */

export type AppScreenNavigationDecision =
  | { readonly kind: "in-app" }
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
        | "subframe-scheme";
    };

export interface AppScreenNavigationContext {
  readonly mainFrame: boolean;
}

/** Takosumi Mobile's own deep-link scheme; pages must not navigate to it. */
export const SHELL_SCHEME = "takosumi";

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

  if (!context.mainFrame) {
    return SUBFRAME_WEB_SCHEMES.has(scheme)
      ? { kind: "in-app" }
      : { kind: "block", reason: "subframe-scheme" };
  }

  if (scheme === "https") {
    if (url.username || url.password) {
      return { kind: "block", reason: "user-info" };
    }
    return url.hostname
      ? { kind: "in-app" }
      : { kind: "block", reason: "invalid-url" };
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
    if (url.username || url.password) {
      return { kind: "block", reason: "user-info" };
    }
    return { kind: "external-confirm", reason: "insecure-http", scheme };
  }
  return { kind: "external-confirm", reason: "other-scheme", scheme };
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
