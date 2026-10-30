import {
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
          if (session) {
            const cleanup = onRemoving(session).catch(() => undefined);
            await Promise.race([cleanup, new Promise<void>((resolve) => setTimeout(resolve, waitMs))]);
          }
        }
        await secureStore.delete(key);
      },
    },
  };
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
