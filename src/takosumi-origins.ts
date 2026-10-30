import {
  mobileAccountListStorageKey,
  type MobilePersistentStore,
  type MobileProductAdapter,
} from "@takosjp/mobile-kit";
import type { AppScreenOpenOptions } from "./app-screens.ts";
import { httpsOrigin } from "./url-policy.ts";

/**
 * Every Takosumi origin this device knows: the Accounts (issuer) and instance
 * origins of each session seen, and the instance of every saved account. App
 * screens show only the sign-in pages of all of them, whichever instance is
 * current, so another saved instance's dashboard never runs in an app screen
 * either. Stored with the non-credential values, never sent anywhere.
 */
export const KNOWN_TAKOSUMI_ORIGINS_STORAGE_KEY = "takosumi.mobile.takosumi-origins.v1";
/** Most origins kept (and sent to the native side, which accepts 32). */
export const MAX_KNOWN_TAKOSUMI_ORIGINS = 32;

type OriginStore = Pick<MobilePersistentStore, "get" | "set">;

/** Distinct https origins, first ones first, at most the native limit. */
export function mergeTakosumiOrigins(...lists: readonly (readonly string[])[]): readonly string[] {
  const merged: string[] = [];
  for (const list of lists) {
    for (const value of list) {
      const origin = httpsOrigin(value);
      if (origin && origin === value && !merged.includes(origin)) merged.push(origin);
      if (merged.length >= MAX_KNOWN_TAKOSUMI_ORIGINS) return merged;
    }
  }
  return merged;
}

async function readStored(store: OriginStore | undefined): Promise<readonly string[]> {
  try {
    const raw = await store?.get(KNOWN_TAKOSUMI_ORIGINS_STORAGE_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : undefined;
    return Array.isArray(parsed)
      ? mergeTakosumiOrigins(parsed.filter((item): item is string => typeof item === "string"))
      : [];
  } catch {
    return [];
  }
}

/** The instance origins of the saved accounts (mobile-kit's account list). */
export async function savedAccountOrigins(
  store: OriginStore | undefined,
  adapter: MobileProductAdapter,
): Promise<readonly string[]> {
  try {
    const raw = await store?.get(mobileAccountListStorageKey(adapter));
    const parsed: unknown = raw ? JSON.parse(raw) : undefined;
    const accounts =
      typeof parsed === "object" && parsed !== null && Array.isArray((parsed as { accounts?: unknown }).accounts)
        ? ((parsed as { accounts: unknown[] }).accounts)
        : [];
    return mergeTakosumiOrigins(
      accounts.flatMap((account) => {
        const host =
          typeof account === "object" && account !== null
            ? (account as { hostUrl?: unknown }).hostUrl
            : undefined;
        const origin = typeof host === "string" ? httpsOrigin(host) : undefined;
        return origin ? [origin] : [];
      }),
    );
  } catch {
    return [];
  }
}

export async function readKnownTakosumiOrigins(input: {
  readonly store: OriginStore | undefined;
  readonly adapter: MobileProductAdapter;
}): Promise<readonly string[]> {
  return mergeTakosumiOrigins(
    await readStored(input.store),
    await savedAccountOrigins(input.store, input.adapter),
  );
}

/** Keep `origins` (a session's issuer and instance) first in the stored list. */
export async function rememberTakosumiOrigins(
  store: OriginStore | undefined,
  origins: readonly string[],
): Promise<readonly string[]> {
  if (!store) return mergeTakosumiOrigins(origins);
  const next = mergeTakosumiOrigins(origins, await readStored(store));
  try {
    await store.set(KNOWN_TAKOSUMI_ORIGINS_STORAGE_KEY, JSON.stringify(next));
  } catch {
    // The list is rebuilt from the sessions seen next time.
  }
  return next;
}

/**
 * Opens `takosumi://app?url=` links in an app screen with every known
 * Takosumi origin, so a dashboard link opens in the browser view rather than
 * as an app. While saved accounts exist but no session is restored yet (a cold
 * start before unlock), a link waits for the session, whose issuer may not be
 * known yet; without saved accounts it opens at once with the origins known.
 */
export function createDeepLinkOpener(input: {
  readonly openUrl: (url: string, options: AppScreenOpenOptions) => Promise<void>;
  readonly platform: string;
  /** The current instance's Takosumi origins, once its session is restored. */
  readonly currentOrigins: () => readonly string[] | undefined;
  readonly knownOrigins: () => Promise<readonly string[]>;
  readonly hasSavedAccounts: () => Promise<boolean>;
  readonly onError?: (error: unknown) => void;
}): { readonly open: (url: string) => Promise<void>; readonly sessionReady: () => Promise<void> } {
  let waiting: string | undefined;
  const openNow = async (url: string) => {
    const accountsOrigins = mergeTakosumiOrigins(input.currentOrigins() ?? [], await input.knownOrigins());
    // No scope: a deep-linked page never becomes the app's remembered last page.
    await input.openUrl(url, { platform: input.platform, accountsOrigins });
  };
  return {
    async open(url) {
      try {
        if (input.currentOrigins() === undefined && (await input.hasSavedAccounts())) {
          waiting = url;
          return;
        }
        await openNow(url);
      } catch (error) {
        input.onError?.(error);
      }
    },
    async sessionReady() {
      const url = waiting;
      waiting = undefined;
      if (url === undefined) return;
      try {
        await openNow(url);
      } catch (error) {
        input.onError?.(error);
      }
    },
  };
}
