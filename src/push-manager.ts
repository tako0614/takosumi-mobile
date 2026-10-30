/**
 * Takosumi Mobile's own notifications, per (instance, account): turning them
 * on and off, keeping the relay subscription, the native key pair and the
 * instance's pusher in step, and noticing when any of them changed. Design:
 * takos-control `docs/platform/takosumi-app-clients.md`「Takosumi Mobile側」.
 *
 * Turning on: relay device (once per device; the secret goes to the secure
 * store) → the instance's `web_push_public_key` → a relay subscription with
 * that key → a native P-256 key pair and `auth` → the instance's pusher
 * (`pushkey` = p256dh, `data.url` = endpoint, `data.auth`).
 */

import {
  mobileAccountIdentity,
  normalizeHostUrl,
  type MobilePersistentStore,
  type MobileSecureStore,
  type MobileSession,
} from "@takosjp/mobile-kit";
import {
  deletePusher,
  getPusherConfig,
  InstancePushError,
  registerPusher,
  type InstanceFetch,
  type WebPushUnavailableReason,
} from "./push-instance.ts";
import { PushUnavailableError, type PushNative, type PushToken } from "./push-native.ts";
import { createRelayClient, RelayError, type RelayDevice, type RelayFetch } from "./push-relay.ts";

export const PUSH_STATE_STORAGE_KEY = "takosumi.mobile.push.v1";
export const RELAY_DEVICE_STORAGE_KEY = "takosumi.mobile.push.relay-device.v1";
/** The relay forgets devices idle for 180 days; touch it well before that. */
const DEVICE_TOUCH_INTERVAL_MS = 7 * 24 * 60 * 60 * 1000;
const MAX_PENDING_REMOVALS = 64;
const MAX_PROMPTED = 64;

/** One (instance, account) with notifications on, on this device. */
export interface PushInstanceRecord {
  /** mobile-kit's account identity: host origin + NUL + account subject. */
  readonly identity: string;
  readonly hostUrl: string;
  readonly accountId?: string;
  readonly label: string;
  readonly subscriptionId: string;
  readonly endpoint: string;
  readonly relayOrigin: string;
  /** The instance's `web_push_public_key` this subscription was made for. */
  readonly instanceKey: string;
  readonly pusherId: string;
  readonly createdAt: string;
  /** The relay subscription is known to be gone (the device registered again). */
  readonly stale?: boolean;
}

interface PushState {
  readonly instances: readonly PushInstanceRecord[];
  /** Identities already asked once after sign-in. */
  readonly prompted: readonly string[];
  /** Relay subscriptions to delete once the relay can be reached. */
  readonly pendingRemovals: readonly { readonly relayOrigin: string; readonly subscriptionId: string }[];
  readonly deviceTouchedAt?: string;
}

export type PushProblem =
  | "unsupported"
  | "relay_not_built"
  | "no_secure_storage"
  | "push_unavailable"
  | "permission_denied"
  | "instance_secret_not_configured"
  | "instance_relay_not_configured"
  | "instance_issuer_not_https"
  | "relay_not_allowed"
  | "pusher_limit"
  | "credential_not_account_wide"
  | "relay_failed"
  | "instance_failed";

export class PushError extends Error {
  readonly problem: PushProblem;

  constructor(problem: PushProblem, cause?: unknown) {
    super(pushProblemMessage(problem));
    this.name = "PushError";
    this.problem = problem;
    if (cause !== undefined) (this as { cause?: unknown }).cause = cause;
  }
}

export function pushProblemMessage(problem: PushProblem): string {
  switch (problem) {
    case "unsupported":
      return "通知はiPhone・AndroidのTakosumi Mobileで受け取れます。";
    case "relay_not_built":
      return "このTakosumi Mobileのビルドには通知の中継が設定されていません。";
    case "no_secure_storage":
      return "この端末では通知の鍵を安全に保存できないため、通知を使えません。";
    case "push_unavailable":
      return "この端末やビルドでは通知の配信（FCM・APNs）を使えません。";
    case "permission_denied":
      return "通知が許可されていません。端末の設定でTakosumiの通知を許可してください。";
    case "instance_secret_not_configured":
      return "このTakosumiは通知を送る準備ができていません（運用者の設定）。";
    case "instance_relay_not_configured":
      return "このTakosumiには通知の中継が設定されていません。運用者に TAKOSUMI_NOTIFICATION_PUSH_RELAY_ORIGINS の設定を依頼してください。";
    case "instance_issuer_not_https":
      return "このTakosumiはhttpsで公開されていないため、通知を送れません。";
    case "relay_not_allowed":
      return "このTakosumiは、このアプリの通知の中継を許可していません。運用者に確認してください。";
    case "pusher_limit":
      return "このアカウントに登録できる端末の数に達しています。Takosumiのアカウント設定で使わない端末を削除してください。";
    case "credential_not_account_wide":
      return "このサインインでは通知を登録できません。サインインし直してください。";
    case "relay_failed":
      return "通知の中継に接続できませんでした。時間をおいてもう一度お試しください。";
    case "instance_failed":
      return "Takosumiに通知の端末を登録できませんでした。時間をおいてもう一度お試しください。";
  }
}

/** What a check of the current instance found and did. */
export type PushCheckResult =
  | { readonly kind: "off" }
  | { readonly kind: "ok" }
  | { readonly kind: "resubscribed" }
  /** The pusher is gone on the instance (removed from its account settings). */
  | { readonly kind: "removed" }
  | { readonly kind: "unavailable"; readonly problem: PushProblem }
  | { readonly kind: "unknown" };

export interface PushManagerDeps {
  /** Undefined outside iOS/Android. */
  readonly native: PushNative | undefined;
  /** The relay origin this build uses; undefined when none was configured. */
  readonly relayOrigin: string | undefined;
  readonly fetch: RelayFetch & InstanceFetch;
  /** Non-secret state (which instances, prompts). */
  readonly store: Pick<MobilePersistentStore, "get" | "set" | "delete"> | undefined;
  /** The relay device secret. */
  readonly secureStore: Pick<MobileSecureStore, "kind" | "get" | "set" | "delete"> | undefined;
  /** A session with a usable access token (mobile-kit's ensureFreshMobileSession). */
  readonly freshSession: (session: MobileSession) => Promise<MobileSession>;
  /** The device language (`ja-JP`) for the instance's texts. */
  readonly lang: () => string;
  readonly now?: () => Date;
}

/** The origins a notification for this session may open: instance and issuer. */
export function pushOrigins(session: Pick<MobileSession, "hostUrl" | "oidcIssuer">): readonly string[] {
  const origins = [normalizeHostUrl(session.hostUrl)];
  try {
    if (session.oidcIssuer) {
      const issuer = new URL(session.oidcIssuer).origin;
      if (/^https?:\/\//u.test(issuer) && !origins.includes(issuer)) origins.push(issuer);
    }
  } catch {
    // An unparsable issuer adds nothing.
  }
  return origins;
}

/** The channel name: the instance's host, and the account when it has a label. */
export function pushLabel(session: Pick<MobileSession, "hostUrl" | "accountLabel">): string {
  const host = new URL(normalizeHostUrl(session.hostUrl)).host;
  const account = session.accountLabel?.trim();
  return (account ? `${host} (${account})` : host).slice(0, 128);
}

export function createPushManager(deps: PushManagerDeps) {
  const now = () => deps.now?.() ?? new Date();
  const relay = deps.relayOrigin ? createRelayClient({ origin: deps.relayOrigin, fetch: deps.fetch }) : undefined;
  let tail: Promise<unknown> = Promise.resolve();
  const serial = <T>(task: () => Promise<T>): Promise<T> => {
    const run = tail.then(task, task);
    tail = run.catch(() => undefined);
    return run;
  };

  const availability = (): PushProblem | undefined => {
    if (!deps.native) return "unsupported";
    if (!relay) return "relay_not_built";
    if (deps.secureStore?.kind !== "secure") return "no_secure_storage";
    return undefined;
  };
  const requireReady = () => {
    const problem = availability();
    if (problem) throw new PushError(problem);
    return { native: deps.native!, relay: relay!, secureStore: deps.secureStore! };
  };

  // --- persistence ---------------------------------------------------------

  async function loadState(): Promise<PushState> {
    try {
      const raw = await deps.store?.get(PUSH_STATE_STORAGE_KEY);
      const parsed: unknown = raw ? JSON.parse(raw) : undefined;
      return normalizeState(parsed);
    } catch {
      return { instances: [], prompted: [], pendingRemovals: [] };
    }
  }

  async function saveState(state: PushState): Promise<void> {
    await deps.store?.set(PUSH_STATE_STORAGE_KEY, JSON.stringify({ version: 1, ...state }));
  }

  async function updateState(change: (state: PushState) => PushState): Promise<PushState> {
    const next = change(await loadState());
    await saveState(next);
    return next;
  }

  async function loadDevice(): Promise<RelayDevice | undefined> {
    const raw = await deps.secureStore?.get(RELAY_DEVICE_STORAGE_KEY);
    if (!raw) return undefined;
    try {
      const device = JSON.parse(raw) as RelayDevice;
      if (
        typeof device.relayOrigin !== "string" ||
        typeof device.deviceId !== "string" ||
        typeof device.deviceSecret !== "string" ||
        (device.platform !== "fcm" && device.platform !== "apns") ||
        typeof device.token !== "string"
      ) {
        return undefined;
      }
      return device;
    } catch {
      return undefined;
    }
  }

  async function saveDevice(device: RelayDevice): Promise<void> {
    await deps.secureStore?.set(RELAY_DEVICE_STORAGE_KEY, JSON.stringify(device));
  }

  // --- relay device ----------------------------------------------------------

  /** Registers a new relay device; the old device's subscriptions are gone. */
  async function registerDevice(token: PushToken): Promise<RelayDevice> {
    const { relay } = requireReady();
    const device = await relay.registerDevice(token);
    await saveDevice(device);
    await updateState((state) => ({
      ...state,
      instances: state.instances.map((record) => ({ ...record, stale: true })),
      // The old device and everything on it are unreachable now.
      pendingRemovals: [],
      deviceTouchedAt: now().toISOString(),
    }));
    return device;
  }

  /** The relay device for this token: registered once, its token kept current. */
  async function ensureDevice(token: PushToken): Promise<RelayDevice> {
    const { relay } = requireReady();
    const device = await loadDevice();
    if (!device || device.relayOrigin !== relay.origin || device.platform !== token.provider) {
      return await registerDevice(token);
    }
    const sameToken =
      device.token === token.token &&
      (token.provider !== "apns" || device.apnsEnvironment === token.environment);
    const state = await loadState();
    const touchedAt = state.deviceTouchedAt ? Date.parse(state.deviceTouchedAt) : 0;
    if (sameToken && now().getTime() - touchedAt < DEVICE_TOUCH_INTERVAL_MS) return device;
    try {
      const updated = await relay.updateDevice(device, token);
      await saveDevice(updated);
      await updateState((current) => ({ ...current, deviceTouchedAt: now().toISOString() }));
      return updated;
    } catch (error) {
      if (error instanceof RelayError && error.deviceUnknown) return await registerDevice(token);
      throw error;
    }
  }

  async function currentToken(): Promise<PushToken> {
    const { native } = requireReady();
    try {
      return await native.getToken();
    } catch (error) {
      // No Firebase configuration, no APNs entitlement, no Play services, or
      // the provider did not answer: this device cannot receive pushes now.
      throw new PushError("push_unavailable", error instanceof PushUnavailableError ? undefined : error);
    }
  }

  // --- subscriptions -----------------------------------------------------------

  async function subscribe(
    session: MobileSession,
    instanceKey: string,
    token: PushToken,
  ): Promise<PushInstanceRecord> {
    const { native, relay } = requireReady();
    let device: RelayDevice;
    let subscription: { subscriptionId: string; endpoint: string };
    try {
      device = await ensureDevice(token);
      try {
        subscription = await relay.createSubscription(device, instanceKey);
      } catch (error) {
        if (!(error instanceof RelayError)) throw error;
        if (error.deviceUnknown) device = await registerDevice(token);
        else if (error.deviceTokenInvalid) device = await relay.updateDevice(device, token);
        else throw error;
        await saveDevice(device);
        subscription = await relay.createSubscription(device, instanceKey);
      }
    } catch (error) {
      if (error instanceof PushError) throw error;
      throw new PushError("relay_failed", error);
    }

    const hostUrl = normalizeHostUrl(session.hostUrl);
    const undoRelay = () => relay.deleteSubscription(device, subscription.subscriptionId).catch(() => undefined);
    let keys: { p256dh: string; auth: string };
    try {
      keys = await native.createSubscriptionKeys({
        subscriptionId: subscription.subscriptionId,
        instance: hostUrl,
        account: session.accountId ?? "",
        label: pushLabel(session),
        origins: pushOrigins(session),
      });
    } catch (error) {
      await undoRelay();
      throw new PushError("push_unavailable", error);
    }

    try {
      const pusher = await registerPusher(session, deps.fetch, {
        pushkey: keys.p256dh,
        endpoint: subscription.endpoint,
        auth: keys.auth,
        deviceDisplayName: await native.deviceName().catch(() => "Takosumi Mobile"),
        lang: deps.lang(),
      });
      return {
        identity: mobileAccountIdentity(session),
        hostUrl,
        ...(session.accountId ? { accountId: session.accountId } : {}),
        label: pushLabel(session),
        subscriptionId: subscription.subscriptionId,
        endpoint: subscription.endpoint,
        relayOrigin: relay.origin,
        instanceKey,
        pusherId: pusher.id,
        createdAt: now().toISOString(),
      };
    } catch (error) {
      await native.deleteSubscriptionKeys(subscription.subscriptionId).catch(() => undefined);
      await undoRelay();
      throw new PushError(registrationProblem(error), error);
    }
  }

  /**
   * Removes one subscription everywhere it can: the instance's pusher (when
   * a session is at hand), the relay subscription and the native keys. A
   * relay that cannot be reached gets the deletion later. Without the
   * pusher deletion, the instance removes it itself on the relay's 404.
   */
  async function removeSubscription(record: PushInstanceRecord, session: MobileSession | undefined): Promise<void> {
    const { native, relay } = requireReady();
    if (session) await deletePusher(session, deps.fetch, record.pusherId).catch(() => undefined);
    let pending = false;
    const device = await loadDevice();
    if (!record.stale && device && device.relayOrigin === record.relayOrigin && record.relayOrigin === relay.origin) {
      try {
        await relay.deleteSubscription(device, record.subscriptionId);
      } catch (error) {
        pending = !(error instanceof RelayError && error.deviceUnknown);
      }
    }
    await native.deleteSubscriptionKeys(record.subscriptionId).catch(() => undefined);
    await updateState((state) => ({
      ...state,
      instances: state.instances.filter((item) => item.subscriptionId !== record.subscriptionId),
      pendingRemovals: pending
        ? [...state.pendingRemovals, { relayOrigin: record.relayOrigin, subscriptionId: record.subscriptionId }].slice(
            -MAX_PENDING_REMOVALS,
          )
        : state.pendingRemovals,
    }));
  }

  /**
   * Whether the relay still knows this device (`PUT /v1/devices/{id}` with
   * the current token): `forgotten` on 401 or without a device for this
   * relay, `unknown` when the relay or the token cannot be reached.
   */
  async function probeDevice(): Promise<"alive" | "forgotten" | "unknown"> {
    const { relay } = requireReady();
    let token: PushToken;
    try {
      token = await currentToken();
    } catch {
      return "unknown";
    }
    const device = await loadDevice();
    if (!device || device.relayOrigin !== relay.origin || device.platform !== token.provider) return "forgotten";
    try {
      const updated = await relay.updateDevice(device, token);
      await saveDevice(updated);
      await updateState((state) => ({ ...state, deviceTouchedAt: now().toISOString() }));
      return "alive";
    } catch (error) {
      return error instanceof RelayError && error.deviceUnknown ? "forgotten" : "unknown";
    }
  }

  async function instanceKeyFor(session: MobileSession): Promise<{ key: string } | { problem: PushProblem }> {
    let config;
    try {
      config = await getPusherConfig(session, deps.fetch);
    } catch (error) {
      if (error instanceof InstancePushError && error.code === "account_wide_credential_required") {
        return { problem: "credential_not_account_wide" };
      }
      throw new PushError("instance_failed", error);
    }
    if (!config.webPushPublicKey) return { problem: unavailableProblem(config.unavailableReason) };
    return { key: config.webPushPublicKey };
  }

  async function findRecord(session: MobileSession): Promise<PushInstanceRecord | undefined> {
    const identity = mobileAccountIdentity(session);
    return (await loadState()).instances.find((item) => item.identity === identity);
  }

  // Every read and write of the state goes through `serial`, so a read never
  // sees the middle of a change (turning on, a check, a sign-out).
  return {
    /** Why notifications cannot work in this app at all, if they cannot. */
    availability,

    record(session: MobileSession): Promise<PushInstanceRecord | undefined> {
      return serial(() => findRecord(session));
    },

    isEnabled(session: MobileSession): Promise<boolean> {
      return serial(async () => (await findRecord(session)) !== undefined);
    },

    /** Turns notifications on for this session's instance and account. */
    enable(session: MobileSession): Promise<PushInstanceRecord> {
      return serial(async () => {
        const { native } = requireReady();
        if ((await native.permissionState()) !== "granted" && !(await native.requestPermission())) {
          throw new PushError("permission_denied");
        }
        const fresh = await deps.freshSession(session);
        const instance = await instanceKeyFor(fresh);
        if ("problem" in instance) throw new PushError(instance.problem);
        const token = await currentToken();
        const previous = await findRecord(fresh);
        const next = await subscribe(fresh, instance.key, token);
        if (previous) await removeSubscription(previous, fresh);
        await updateState((state) => ({
          ...state,
          instances: [...state.instances.filter((item) => item.identity !== next.identity), next],
          prompted: rememberPrompt(state.prompted, next.identity),
        }));
        return next;
      });
    },

    /** Turns notifications off for this session's instance and account. */
    disable(session: MobileSession): Promise<void> {
      return serial(async () => {
        const current = await findRecord(session);
        if (!current) return;
        const fresh = await deps.freshSession(session).catch(() => session);
        await removeSubscription(current, fresh);
      });
    },

    /**
     * On launch and whenever this instance becomes current. A pusher gone
     * from the instance while this device's relay subscription stands means
     * the person removed this device in the account settings: turn off, never
     * quietly register again — unless the relay no longer knows this device
     * (asked with `PUT /v1/devices/{id}`), which is why the instance dropped
     * the pusher. Resubscribe then, and when the instance's key changed (its
     * secret was rotated), the relay subscription is known to be gone, or the
     * native keys are gone.
     */
    check(session: MobileSession): Promise<PushCheckResult> {
      return serial(async (): Promise<PushCheckResult> => {
        const current = await findRecord(session);
        if (!current) return { kind: "off" };
        const problem = availability();
        if (problem) return { kind: "unavailable", problem };
        const { native, relay } = requireReady();
        let fresh: MobileSession;
        let config;
        try {
          fresh = await deps.freshSession(session);
          config = await getPusherConfig(fresh, deps.fetch);
        } catch {
          return { kind: "unknown" };
        }
        if (!config.webPushPublicKey) {
          return { kind: "unavailable", problem: unavailableProblem(config.unavailableReason) };
        }
        let stale = current.stale === true;
        if (!stale && !config.pushers.some((pusher) => pusher.id === current.pusherId)) {
          // Before concluding that the person removed this device, ask the
          // relay whether it still knows it: a relay that forgot the device
          // (purged after 180 idle days) answered the instance 404, and the
          // instance dropped the pusher. That is a subscription to renew.
          const device = await probeDevice();
          if (device === "unknown") return { kind: "unknown" };
          if (device === "alive") {
            await removeSubscription(current, undefined);
            return { kind: "removed" };
          }
          stale = true;
        }
        const nativeKeys = await native.listSubscriptions().catch(() => undefined);
        const keysMissing = nativeKeys !== undefined && !nativeKeys.some((item) => item.subscriptionId === current.subscriptionId);
        if (
          stale ||
          keysMissing ||
          config.webPushPublicKey !== current.instanceKey ||
          current.relayOrigin !== relay.origin
        ) {
          try {
            const next = await subscribe(fresh, config.webPushPublicKey, await currentToken());
            await removeSubscription(current, fresh);
            await updateState((state) => ({
              ...state,
              instances: [...state.instances.filter((item) => item.identity !== next.identity), next],
            }));
            return { kind: "resubscribed" };
          } catch (error) {
            return { kind: "unavailable", problem: error instanceof PushError ? error.problem : "instance_failed" };
          }
        }
        // Keep the relay's copy of the token current (and the device alive).
        try {
          await ensureDevice(await currentToken());
        } catch {
          // A token problem shows up the next time; the subscription stands.
        }
        return { kind: "ok" };
      });
    },

    /**
     * Once per launch with every saved account: drops subscriptions of
     * accounts that are gone, native keys nothing refers to, and relay
     * deletions that failed before.
     */
    reconcile(saved: readonly Pick<MobileSession, "hostUrl" | "accountId">[]): Promise<void> {
      return serial(async () => {
        if (availability()) return;
        const { native, relay } = requireReady();
        const identities = new Set(saved.map((account) => mobileAccountIdentity(account)));
        const state = await loadState();
        for (const item of state.instances) {
          if (!identities.has(item.identity)) await removeSubscription(item, undefined);
        }
        const known = new Set((await loadState()).instances.map((item) => item.subscriptionId));
        const device = await loadDevice();
        for (const orphan of await native.listSubscriptions().catch(() => [])) {
          if (known.has(orphan.subscriptionId)) continue;
          await native.deleteSubscriptionKeys(orphan.subscriptionId).catch(() => undefined);
          if (device && device.relayOrigin === relay.origin) {
            await relay.deleteSubscription(device, orphan.subscriptionId).catch(() => undefined);
          }
        }
        const pending = (await loadState()).pendingRemovals;
        const left: { relayOrigin: string; subscriptionId: string }[] = [];
        for (const removal of pending) {
          if (!device || removal.relayOrigin !== device.relayOrigin || removal.relayOrigin !== relay.origin) continue;
          try {
            await relay.deleteSubscription(device, removal.subscriptionId);
          } catch (error) {
            if (!(error instanceof RelayError && error.deviceUnknown)) left.push(removal);
          }
        }
        await updateState((current) => ({ ...current, pendingRemovals: left }));
      });
    },

    /** The APNs/FCM token changed: tell the relay (`PUT /v1/devices/{id}`). */
    tokenRefreshed(token: PushToken): Promise<void> {
      return serial(async () => {
        if (availability()) return;
        if (!(await loadDevice())) return;
        if ((await loadState()).instances.length === 0) return;
        await ensureDevice(token).catch(() => undefined);
      });
    },

    /**
     * A saved session is about to be deleted (sign-out): remove its pusher
     * with the session it still has, its relay subscription and its keys.
     */
    accountRemoving(session: MobileSession): Promise<void> {
      return serial(async () => {
        if (availability()) return;
        const current = await findRecord(session);
        if (current) await removeSubscription(current, session);
      });
    },

    /** Ask once after sign-in, unless notifications are on or cannot work. */
    shouldPrompt(session: MobileSession): Promise<boolean> {
      return serial(async () => {
        if (availability()) return false;
        const state = await loadState();
        const identity = mobileAccountIdentity(session);
        return !state.prompted.includes(identity) && !state.instances.some((item) => item.identity === identity);
      });
    },

    markPrompted(session: MobileSession): Promise<void> {
      return serial(async () => {
        const identity = mobileAccountIdentity(session);
        await updateState((state) => ({ ...state, prompted: rememberPrompt(state.prompted, identity) }));
      });
    },
  };
}

export type PushManager = ReturnType<typeof createPushManager>;

function rememberPrompt(prompted: readonly string[], identity: string): readonly string[] {
  return [...prompted.filter((item) => item !== identity), identity].slice(-MAX_PROMPTED);
}

function unavailableProblem(reason: WebPushUnavailableReason | null): PushProblem {
  switch (reason) {
    case "relay_not_configured":
      return "instance_relay_not_configured";
    case "issuer_not_https":
      return "instance_issuer_not_https";
    default:
      return "instance_secret_not_configured";
  }
}

function registrationProblem(error: unknown): PushProblem {
  if (!(error instanceof InstancePushError)) return "instance_failed";
  switch (error.code) {
    case "invalid_pusher_endpoint":
      return "relay_not_allowed";
    case "notification_pusher_limit_reached":
      return "pusher_limit";
    case "notification_relay_not_configured":
      return "instance_relay_not_configured";
    case "notification_issuer_not_https":
      return "instance_issuer_not_https";
    case "web_push_unavailable":
      return "instance_secret_not_configured";
    case "account_wide_credential_required":
      return "credential_not_account_wide";
    default:
      return "instance_failed";
  }
}

function normalizeState(value: unknown): PushState {
  const body = typeof value === "object" && value !== null ? (value as Record<string, unknown>) : {};
  const instances = Array.isArray(body.instances)
    ? body.instances.filter((item): item is PushInstanceRecord => {
        const record = item as Partial<PushInstanceRecord> | null;
        return (
          typeof record === "object" &&
          record !== null &&
          typeof record.identity === "string" &&
          typeof record.hostUrl === "string" &&
          typeof record.subscriptionId === "string" &&
          typeof record.endpoint === "string" &&
          typeof record.relayOrigin === "string" &&
          typeof record.instanceKey === "string" &&
          typeof record.pusherId === "string" &&
          typeof record.label === "string"
        );
      })
    : [];
  const prompted = Array.isArray(body.prompted)
    ? body.prompted.filter((item): item is string => typeof item === "string")
    : [];
  const pendingRemovals = Array.isArray(body.pendingRemovals)
    ? body.pendingRemovals.filter(
        (item): item is { relayOrigin: string; subscriptionId: string } =>
          typeof item === "object" &&
          item !== null &&
          typeof (item as { relayOrigin?: unknown }).relayOrigin === "string" &&
          typeof (item as { subscriptionId?: unknown }).subscriptionId === "string",
      )
    : [];
  return {
    instances,
    prompted,
    pendingRemovals,
    ...(typeof body.deviceTouchedAt === "string" ? { deviceTouchedAt: body.deviceTouchedAt } : {}),
  };
}
