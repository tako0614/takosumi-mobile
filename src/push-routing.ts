/**
 * Where a tapped Takosumi notification goes: the saved (instance, account) it
 * was for, then the page its Declarative Web Push `navigate` names. Pages
 * Takosumi Mobile shows itself (a Run) open in the app; any other page of the
 * instance opens in the browser view, like every Takosumi page outside
 * sign-in (#7).
 */

import { mobileAccountIdentity, type MobileSession } from "@takosjp/mobile-kit";
import type { NotificationTap } from "./push-native.ts";

export type NotificationRoute =
  | { readonly kind: "run"; readonly runId: string; readonly url: string }
  | { readonly kind: "page"; readonly url: string }
  | { readonly kind: "home" };

const RUN_ID = /^[A-Za-z0-9_.:-]{1,128}$/u;

/** The route for `navigate`, judged with the session's Takosumi origins. */
export function routeNotification(navigate: string | undefined, origins: readonly string[]): NotificationRoute {
  if (!navigate) return { kind: "home" };
  let url: URL;
  try {
    url = new URL(navigate);
  } catch {
    return { kind: "home" };
  }
  if (url.username || url.password || (url.protocol !== "https:" && url.protocol !== "http:")) {
    return { kind: "home" };
  }
  if (!origins.includes(url.origin)) return { kind: "home" };
  const run = /^\/runs\/([^/]+)\/?$/u.exec(url.pathname);
  if (run) {
    let runId: string;
    try {
      runId = decodeURIComponent(run[1]!);
    } catch {
      runId = "";
    }
    if (RUN_ID.test(runId)) return { kind: "run", runId, url: url.href };
  }
  return { kind: "page", url: url.href };
}

/** A tap names an instance origin and an account subject ("" for none). */
export function tapIdentity(tap: Pick<NotificationTap, "instance" | "account">): string | undefined {
  try {
    return mobileAccountIdentity({ hostUrl: tap.instance, accountId: tap.account || undefined });
  } catch {
    return undefined;
  }
}

/** The saved account a tap belongs to, if it is still saved. */
export function accountForTap<Account extends Pick<MobileSession, "hostUrl" | "accountId">>(
  tap: Pick<NotificationTap, "instance" | "account">,
  accounts: readonly Account[],
): Account | undefined {
  const identity = tapIdentity(tap);
  if (!identity) return undefined;
  return accounts.find((account) => {
    try {
      return mobileAccountIdentity(account) === identity;
    } catch {
      return false;
    }
  });
}

/** True when the tap is for this session's instance and account. */
export function tapIsForSession(
  tap: Pick<NotificationTap, "instance" | "account">,
  session: Pick<MobileSession, "hostUrl" | "accountId">,
): boolean {
  const identity = tapIdentity(tap);
  try {
    return identity !== undefined && identity === mobileAccountIdentity(session);
  } catch {
    return false;
  }
}

/**
 * The notification waiting to open: kept from the tap (or a cold start)
 * until the shell has that instance's session, then taken once.
 */
export interface PendingNotification {
  readonly identity: string;
  readonly navigate?: string;
}

export function createPendingNotification() {
  let pending: PendingNotification | undefined;
  const listeners = new Set<() => void>();
  return {
    set(next: PendingNotification) {
      pending = next;
      for (const listener of listeners) listener();
    },
    /** Takes the pending notification when it belongs to `session`. */
    takeFor(session: Pick<MobileSession, "hostUrl" | "accountId">): PendingNotification | undefined {
      if (!pending || pending.identity !== mobileAccountIdentity(session)) return undefined;
      const taken = pending;
      pending = undefined;
      return taken;
    },
    subscribe(listener: () => void): () => void {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

export type PendingNotifications = ReturnType<typeof createPendingNotification>;
