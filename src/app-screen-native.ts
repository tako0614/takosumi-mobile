import {
  APP_SCREEN_PROFILE,
  type AppScreenOpenRequest,
} from "./app-screens.ts";
import { isLaunchableAppScreenUrl } from "./url-policy.ts";

/**
 * Calls into the product-owned `app-screen` Tauri plugin
 * (`src-tauri/plugins/app-screen`). Outside the iOS/Android runtime (web
 * preview) an app screen falls back to the external browser.
 */

export interface AppScreenCapabilities {
  /** Screens open in Takosumi Mobile's own native webview. */
  readonly inApp: boolean;
  /** The launcher can pin a home-screen shortcut (Android 8+ launchers). */
  readonly pinShortcut: boolean;
}

export interface AppScreenOpener {
  readonly capabilities: () => Promise<AppScreenCapabilities>;
  readonly open: (request: AppScreenOpenRequest) => Promise<void>;
  /** Resolves `true` when the launcher was asked to pin the shortcut. */
  readonly pinShortcut: (request: AppScreenOpenRequest) => Promise<boolean>;
}

export type AppScreenInvoke = <T>(
  command: string,
  args?: Record<string, unknown>,
) => Promise<T>;

export function createAppScreenOpener(input: {
  /** Tauri `invoke` on iOS/Android; `undefined` in the web preview. */
  readonly invoke?: AppScreenInvoke;
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
  };
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
