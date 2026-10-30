import {
  type AppLauncherEntry,
  type AppScreenIcon,
  type AppScreenOpenOptions,
  type AppScreenTarget,
  appScreenOpenRequest,
  resolveAppScreenTarget,
} from "./app-screens.ts";
import type { AppScreenOpener } from "./app-screen-native.ts";
import { type NativeIconSource, nativeIconSource } from "./app-icons.ts";

/**
 * Launcher behavior without UI: open an entry or a deep-linked URL in an app
 * screen, pin a shortcut, and cache icons. The Solid view holds the signals;
 * deep links reach the same controller before the home has rendered.
 */
export interface LauncherController {
  readonly setEntries: (entries: readonly AppLauncherEntry[]) => void;
  readonly entries: () => readonly AppLauncherEntry[];
  /** Open in an app screen (中で見る); the open router decides when. */
  readonly openTarget: (
    target: AppScreenTarget,
    options?: AppScreenOpenOptions,
  ) => Promise<void>;
  /** A deep-linked URL: the known app with that origin, else its own screen. */
  readonly openUrl: (url: string, options?: AppScreenOpenOptions) => Promise<void>;
  readonly pinEntry: (
    entry: AppLauncherEntry,
    options?: AppScreenOpenOptions,
  ) => Promise<boolean>;
  /** Cached `data:` URL of an image icon, for `<img>` in the launcher. */
  readonly iconDataUrl: (icon: AppScreenIcon | undefined) => Promise<string | undefined>;
}

/** How long opening waits for an icon before it opens without one. */
export const ICON_WAIT_MS = 1_500;

export function createLauncherController(input: {
  readonly opener: AppScreenOpener;
  readonly fetchIcon: (url: string) => Promise<string | undefined>;
  readonly rasterize: (source: NativeIconSource) => Promise<string | undefined>;
  readonly iconWaitMs?: number;
}): LauncherController {
  let entries: readonly AppLauncherEntry[] = [];
  const imageCache = new Map<string, Promise<string | undefined>>();
  const pngCache = new Map<string, Promise<string | undefined>>();
  const iconWaitMs = input.iconWaitMs ?? ICON_WAIT_MS;

  const iconDataUrl = (icon: AppScreenIcon | undefined) => {
    if (icon?.kind !== "image") return Promise.resolve(undefined);
    let cached = imageCache.get(icon.url);
    if (!cached) {
      cached = input.fetchIcon(icon.url).catch(() => undefined);
      imageCache.set(icon.url, cached);
    }
    return cached;
  };

  const iconPng = (target: AppScreenTarget) => {
    const cacheKey = `${target.key}\u0000${
      target.icon?.kind === "image"
        ? target.icon.url
        : target.icon?.kind === "glyph"
          ? target.icon.glyph
          : target.fallbackGlyph
    }`;
    let cached = pngCache.get(cacheKey);
    if (!cached) {
      cached = iconDataUrl(target.icon)
        .then((loaded) =>
          input.rasterize(nativeIconSource(target.icon, loaded, target.fallbackGlyph)),
        )
        .catch(() => undefined);
      pngCache.set(cacheKey, cached);
    }
    return cached;
  };

  const withinBudget = <T>(promise: Promise<T | undefined>) =>
    Promise.race([
      promise,
      new Promise<undefined>((resolve) => setTimeout(() => resolve(undefined), iconWaitMs)),
    ]);

  const open = async (target: AppScreenTarget, options?: AppScreenOpenOptions) => {
    const png = await withinBudget(iconPng(target));
    await input.opener.open(appScreenOpenRequest(target, png, options));
  };

  return {
    setEntries(next) {
      entries = next;
    },
    entries: () => entries,
    iconDataUrl,
    async openTarget(target, options) {
      await open(target, options);
    },
    async openUrl(url, options) {
      await open(resolveAppScreenTarget(url, entries), options);
    },
    async pinEntry(entry, options) {
      const png = await withinBudget(iconPng(entry));
      return await input.opener.pinShortcut(appScreenOpenRequest(entry, png, options));
    },
  };
}
