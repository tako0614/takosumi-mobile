import {
  mobileAccountIdentity,
  mobileAccountListStorageKey,
  mobileSessionStorageKey,
  type MobileProductAdapter,
  type MobileSession,
  type NativeBridge,
} from "@takosjp/mobile-kit";

/** How long a sign-out waits for the pusher and relay cleanup. */
export const ACCOUNT_REMOVAL_WAIT_MS = 4_000;

/**
 * Tells `onRemoving` about a saved session just before mobile-kit deletes its
 * credential slot (sign-out, a refresh the issuer refused, an evicted
 * account), with the session as it was stored, so its notification pusher
 * and relay subscription can go while its token may still work. The deletion
 * itself waits at most {@link ACCOUNT_REMOVAL_WAIT_MS}; cleanup that takes
 * longer continues on its own, and the instance deletes a pusher whose relay
 * endpoint answers 404 anyway.
 *
 * Only a slot whose stored session belongs to the account row it is filed
 * under counts. mobile-kit also drops a slot whose stored session names
 * another host or account; that session's account may still be saved under
 * its own slot, and its notifications must stay.
 */
export function withAccountRemovalHook(
  bridge: NativeBridge,
  adapter: MobileProductAdapter,
  onRemoving: (session: MobileSession) => Promise<void>,
  waitMs = ACCOUNT_REMOVAL_WAIT_MS,
): NativeBridge {
  const secureStore = bridge.secureStore;
  if (!secureStore) return bridge;
  const slotPrefix = `${mobileSessionStorageKey(adapter)}.`;
  return {
    ...bridge,
    secureStore: {
      ...secureStore,
      async delete(key: string) {
        if (key.startsWith(slotPrefix)) {
          const session = await secureStore.get(key).then(parseStoredSession, () => undefined);
          if (session && (await filedUnderOwnAccount(bridge, adapter, key.slice(slotPrefix.length), session))) {
            const cleanup = onRemoving(session).catch(() => undefined);
            await Promise.race([cleanup, new Promise<void>((resolve) => setTimeout(resolve, waitMs))]);
          }
        }
        await secureStore.delete(key);
      },
    },
  };
}

/** True when the account row with this slot id is the session's own account. */
async function filedUnderOwnAccount(
  bridge: NativeBridge,
  adapter: MobileProductAdapter,
  slotId: string,
  session: MobileSession,
): Promise<boolean> {
  try {
    const raw = await bridge.storage?.get(mobileAccountListStorageKey(adapter));
    const list = raw ? (JSON.parse(raw) as { accounts?: unknown }) : undefined;
    const rows = Array.isArray(list?.accounts) ? list.accounts : [];
    const row = rows.find(
      (item): item is { id: string; hostUrl: string; accountId?: string } =>
        typeof item === "object" &&
        item !== null &&
        (item as { id?: unknown }).id === slotId &&
        typeof (item as { hostUrl?: unknown }).hostUrl === "string",
    );
    if (!row) return false;
    return mobileAccountIdentity({ hostUrl: row.hostUrl, accountId: row.accountId }) === mobileAccountIdentity(session);
  } catch {
    return false;
  }
}

function parseStoredSession(raw: string | undefined): MobileSession | undefined {
  if (!raw) return undefined;
  try {
    const value = JSON.parse(raw) as Partial<MobileSession>;
    if (
      typeof value.hostUrl !== "string" ||
      typeof value.accessToken !== "string" ||
      typeof value.tokenType !== "string" ||
      typeof value.product !== "string"
    ) {
      return undefined;
    }
    return value as MobileSession;
  } catch {
    return undefined;
  }
}
