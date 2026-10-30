import type { OpenClientRequest } from "./app-clients.ts";
import {
  APP_SCREEN_PROFILE,
  type AppScreenOpenRequest,
} from "./app-screens.ts";
import { isLaunchableAppScreenUrl } from "./url-policy.ts";

/**
 * Calls into the product-owned `app-screen` Tauri plugin
 * (`src-tauri/plugins/app-screen`). Outside the iOS/Android runtime (web
 * preview) an app screen and the browser view fall back to the external
 * browser, and a native app is never found.
 */

export interface AppScreenCapabilities {
  /** Screens open in Takosumi Mobile's own native webview. */
  readonly inApp: boolean;
  /** The launcher can pin a home-screen shortcut (Android 8+ launchers). */
  readonly pinShortcut: boolean;
}

/** `open_client`: whether the declared native app took the link. */
export type OpenClientResult = "opened" | "not_installed";

/**
 * `open_browser_view`: Custom Tabs / `SFSafariViewController`, or the normal
 * browser on an Android phone without a Custom Tabs browser.
 */
export type BrowserViewResult = "browser-view" | "browser";

export type AppScreenActionReason = "change" | "not-installed" | "unverified";

/**
 * Something the person asked for in an app screen's bar that the shell
 * handles: show the chooser for that app (開き方を変える, a declared app that
 * is not installed, or an unverified app that needs confirming).
 */
export interface AppScreenAction {
  readonly kind: "choose";
  readonly key: string;
  readonly url: string;
  readonly reason: AppScreenActionReason;
}

export interface AppScreenOpener {
  readonly capabilities: () => Promise<AppScreenCapabilities>;
  readonly open: (request: AppScreenOpenRequest) => Promise<void>;
  /** Resolves `true` when the launcher was asked to pin the shortcut. */
  readonly pinShortcut: (request: AppScreenOpenRequest) => Promise<boolean>;
  /** Hand the screen to its declared native app. */
  readonly openClient: (request: OpenClientRequest) => Promise<OpenClientResult>;
  /** Open an https page in the browser view over Takosumi Mobile. */
  readonly openBrowserView: (url: string) => Promise<BrowserViewResult>;
  /**
   * Open an https page in the phone's own browser (「ブラウザで開く」), never
   * in an app that claims the link (Android; iOS cannot prevent a universal
   * link from opening its app).
   */
  readonly openWeb: (url: string) => Promise<void>;
  /** Take the bar action waiting for the shell, if any (once). */
  readonly takePendingAction: () => Promise<AppScreenAction | undefined>;
  /** Called when an app screen's bar leaves an action for the shell. */
  readonly onActionPending: (handler: () => void) => Promise<() => void>;
}

export type AppScreenInvoke = <T>(
  command: string,
  args?: Record<string, unknown>,
) => Promise<T>;

export type AppScreenListen = (
  plugin: string,
  event: string,
  handler: (payload: unknown) => void,
) => Promise<{ readonly unregister: () => Promise<void> }>;

/** The app screen bar's color; the browser view's toolbar matches it. */
export const APP_SCREEN_BAR_COLOR = "#151517";

export function createAppScreenOpener(input: {
  /** Tauri `invoke` on iOS/Android; `undefined` in the web preview. */
  readonly invoke?: AppScreenInvoke;
  /** Tauri `addPluginListener` on iOS/Android. */
  readonly listen?: AppScreenListen;
  readonly platform: string;
  readonly openExternalUrl: (url: string) => Promise<void>;
}): AppScreenOpener {
  const native = input.invoke;
  if (!native || (input.platform !== "android" && input.platform !== "ios")) {
    return {
      async capabilities() {
        return { inApp: false, pinShortcut: false };
      },
      async open(request) {
        await input.openExternalUrl(checkedRequest(request).url);
      },
      async pinShortcut() {
        return false;
      },
      async openClient() {
        return "not_installed";
      },
      async openBrowserView(url) {
        await input.openExternalUrl(checkedBrowserUrl(url));
        return "browser";
      },
      async openWeb(url) {
        await input.openExternalUrl(checkedBrowserUrl(url));
      },
      async takePendingAction() {
        return undefined;
      },
      async onActionPending() {
        return () => undefined;
      },
    };
  }
  let capabilities: Promise<AppScreenCapabilities> | undefined;
  return {
    capabilities() {
      capabilities ??= native<unknown>("plugin:app-screen|capabilities")
        .then((value) => ({
          inApp: true,
          pinShortcut:
            input.platform === "android" &&
            typeof value === "object" &&
            value !== null &&
            (value as Record<string, unknown>).pinShortcut === true,
        }))
        .catch(() => ({ inApp: true, pinShortcut: false }));
      return capabilities;
    },
    async open(request) {
      await native("plugin:app-screen|open", {
        payload: checkedRequest(request),
      });
    },
    async pinShortcut(request) {
      if (input.platform !== "android") return false;
      const result = await native<unknown>("plugin:app-screen|pin_shortcut", {
        payload: checkedRequest(request),
      });
      return (
        typeof result === "object" &&
        result !== null &&
        (result as Record<string, unknown>).requested === true
      );
    },
    async openClient(request) {
      if (request.platform !== input.platform) return "not_installed";
      const result = await native<unknown>("plugin:app-screen|open_client", {
        payload: request,
      });
      return recordField(result, "result") === "opened" ? "opened" : "not_installed";
    },
    async openBrowserView(url) {
      const result = await native<unknown>("plugin:app-screen|open_browser_view", {
        payload: { url: checkedBrowserUrl(url), toolbarColor: APP_SCREEN_BAR_COLOR },
      });
      return recordField(result, "result") === "browser" ? "browser" : "browser-view";
    },
    async openWeb(url) {
      await native("plugin:app-screen|open_web", { payload: { url: checkedBrowserUrl(url) } });
    },
    async takePendingAction() {
      const result = await native<unknown>("plugin:app-screen|take_pending_action");
      return decodeAction(recordField(result, "action"));
    },
    async onActionPending(handler) {
      if (!input.listen) return () => undefined;
      const listener = await input.listen("app-screen", "actionPending", () => handler());
      return () => {
        void listener.unregister();
      };
    },
  };
}

/** Accept only a well-formed action; the shell resolves its key itself. */
export function decodeAction(value: unknown): AppScreenAction | undefined {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return undefined;
  }
  const record = value as Record<string, unknown>;
  if (record.kind !== "choose") return undefined;
  if (typeof record.key !== "string" || !record.key || record.key.length > 512) {
    return undefined;
  }
  if (typeof record.url !== "string" || !isLaunchableAppScreenUrl(record.url)) {
    return undefined;
  }
  const reason =
    record.reason === "not-installed" || record.reason === "unverified"
      ? record.reason
      : "change";
  return { kind: "choose", key: record.key, url: record.url, reason };
}

function recordField(value: unknown, field: string): unknown {
  return typeof value === "object" && value !== null
    ? (value as Record<string, unknown>)[field]
    : undefined;
}

function checkedRequest(request: AppScreenOpenRequest): AppScreenOpenRequest {
  if (request.profile !== APP_SCREEN_PROFILE) {
    throw new Error("App screens share one browsing profile.");
  }
  if (!isLaunchableAppScreenUrl(request.url)) {
    throw new Error("Only https URLs can be opened in an app screen.");
  }
  return request;
}

function checkedBrowserUrl(url: string): string {
  if (!isLaunchableAppScreenUrl(url)) {
    throw new Error("Only https URLs can be opened in the browser view.");
  }
  return url;
}
