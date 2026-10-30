import type { MobilePersistentStore, MobileSession } from "@takosjp/mobile-kit";
import {
  clientForPlatform,
  openClientRequest,
  type ScreenClient,
  type ScreenClients,
} from "./app-clients.ts";
import type {
  AppScreenAction,
  AppScreenActionReason,
  AppScreenOpener,
} from "./app-screen-native.ts";
import {
  APP_SCREEN_SCOPE_MAX_LENGTH,
  type AppLauncherEntry,
  type AppScreenIcon,
  type AppScreenOpenOptions,
  resolveAppScreenTarget,
} from "./app-screens.ts";
import { isLaunchableAppScreenUrl, screenOrigin, takosumiOrigins } from "./url-policy.ts";

/**
 * How Takosumi Mobile opens an app (TASK-0119 step 3, design
 * `takos-control/docs/platform/takosumi-app-clients.md` §3):
 *
 * 1. the choice remembered on this device for this app, if any;
 * 2. the app's declaration: its native app when the declaration prefers it
 *    and the client for this phone is verified. An unverified client is never
 *    a default and never remembered: the chooser shows its real destination
 *    and opens it only after the person confirms;
 * 3. otherwise the app screen inside Takosumi Mobile (中で見る).
 *
 * A native app that turns out not to be installed brings up the chooser, and
 * the remembered native choice stays for after it is installed.
 */

export type OpenMode = "native" | "web" | "browser-view" | "in-app";

export const OPEN_MODES: readonly OpenMode[] = ["native", "web", "browser-view", "in-app"];

/** What the chooser was opened for. */
export type ChooserReason = "choose" | AppScreenActionReason;

/** An app the router can open: a launcher entry or a deep-linked screen. */
export interface OpenTarget {
  readonly key: string;
  readonly label: string;
  readonly url: string;
  readonly icon?: AppScreenIcon;
  readonly fallbackGlyph: string;
  readonly source?: AppLauncherEntry["source"];
  readonly clients?: ScreenClients;
}

/** The current Takosumi: where choices, recent apps and last pages belong. */
export interface OpenContext {
  /** The instance's origin; remembered choices are kept per instance. */
  readonly instanceOrigin: string;
  /**
   * The Takosumi origins (Accounts and the instance itself): app screens show
   * only their sign-in pages.
   */
  readonly accountsOrigins: readonly string[];
  /**
   * (instance, account), as SHA-256 hex: recent apps and last pages are kept
   * per account. Hashed so a long account id never merges accounts and the
   * device stores no raw subject next to the pages.
   */
  readonly scope: string;
}

export async function openContextFromSession(
  session: MobileSession,
): Promise<OpenContext | undefined> {
  const instanceOrigin = screenOrigin(session.hostUrl);
  if (!instanceOrigin) return undefined;
  return {
    instanceOrigin,
    accountsOrigins: takosumiOrigins(session),
    scope: await openScope(instanceOrigin, session.accountId),
  };
}

/** SHA-256 hex of (instance origin, account subject). */
export async function openScope(instanceOrigin: string, accountId: string | undefined): Promise<string> {
  const bytes = new TextEncoder().encode(`${instanceOrigin}\n${accountId ?? ""}`);
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  const scope = [...digest].map((byte) => byte.toString(16).padStart(2, "0")).join("");
  if (scope.length > APP_SCREEN_SCOPE_MAX_LENGTH) throw new Error("scope is too long");
  return scope;
}

// --- Plan ---------------------------------------------------------------------

export type OpenPlan =
  | { readonly kind: "open"; readonly mode: OpenMode }
  | { readonly kind: "choose"; readonly reason: "unverified" };

export function planOpen(input: {
  /** The declared client for this phone, if any. */
  readonly client?: ScreenClient;
  readonly preferred?: ScreenClients["preferred"];
  readonly remembered?: OpenMode;
}): OpenPlan {
  const { client, remembered } = input;
  if (remembered === "native") {
    if (client?.verified) return { kind: "open", mode: "native" };
    // A remembered native choice never opens an unverified client.
  } else if (remembered !== undefined) {
    return { kind: "open", mode: remembered };
  }
  if (client?.verified && input.preferred === "native") {
    return { kind: "open", mode: "native" };
  }
  if (client && !client.verified) return { kind: "choose", reason: "unverified" };
  return { kind: "open", mode: "in-app" };
}

// --- Remembered choice --------------------------------------------------------

/**
 * Remembered choices, on this device only (the non-credential store), never
 * sent to Takosumi. One per (instance, app, screen) for Takosumi screens and
 * one per added URL.
 */
export const OPEN_CHOICES_STORAGE_KEY = "takosumi.mobile.open-choices.v1";
export const MAX_REMEMBERED_CHOICES = 200;

type ChoiceStore = Pick<MobilePersistentStore, "get" | "set">;

interface StoredChoice {
  readonly mode: OpenMode;
  readonly at: string;
}

export function openChoiceKey(
  target: Pick<OpenTarget, "key" | "source">,
  context: Pick<OpenContext, "instanceOrigin"> | undefined,
): string {
  // Takosumi screens (`app:<capsule>:<interface>`) belong to their instance.
  return target.source === "takosumi" && context
    ? `${context.instanceOrigin}\n${target.key}`
    : `device\n${target.key}`;
}

async function readChoices(store: ChoiceStore | undefined): Promise<Map<string, StoredChoice>> {
  const choices = new Map<string, StoredChoice>();
  const raw = await safeGet(store, OPEN_CHOICES_STORAGE_KEY);
  if (!raw) return choices;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return choices;
  }
  if (!isRecord(parsed) || !isRecord(parsed.choices)) return choices;
  for (const [key, value] of Object.entries(parsed.choices)) {
    if (!isRecord(value)) continue;
    const mode = OPEN_MODES.find((candidate) => candidate === value.mode);
    if (!mode || key.length > 2_048) continue;
    choices.set(key, { mode, at: typeof value.at === "string" ? value.at : "" });
  }
  return choices;
}

async function writeChoices(
  store: ChoiceStore | undefined,
  choices: Map<string, StoredChoice>,
): Promise<void> {
  const kept = [...choices.entries()]
    .sort((a, b) => b[1].at.localeCompare(a[1].at))
    .slice(0, MAX_REMEMBERED_CHOICES);
  await store?.set(
    OPEN_CHOICES_STORAGE_KEY,
    JSON.stringify({ choices: Object.fromEntries(kept) }),
  );
}

export async function readOpenChoice(
  store: ChoiceStore | undefined,
  choiceKey: string,
): Promise<OpenMode | undefined> {
  return (await readChoices(store)).get(choiceKey)?.mode;
}

/**
 * Remember `mode` for this app. `native` is remembered only for a verified
 * client; for anything else nothing is stored and `false` is returned.
 */
export async function rememberOpenChoice(input: {
  readonly store: ChoiceStore | undefined;
  readonly choiceKey: string;
  readonly mode: OpenMode;
  readonly client?: ScreenClient;
  readonly now?: () => Date;
}): Promise<boolean> {
  if (!input.store) return false;
  if (input.mode === "native" && input.client?.verified !== true) return false;
  const choices = await readChoices(input.store);
  choices.set(input.choiceKey, {
    mode: input.mode,
    at: (input.now ?? (() => new Date()))().toISOString(),
  });
  await writeChoices(input.store, choices);
  return true;
}

export async function forgetOpenChoice(
  store: ChoiceStore | undefined,
  choiceKey: string,
): Promise<void> {
  const choices = await readChoices(store);
  if (choices.delete(choiceKey)) await writeChoices(store, choices);
}

// --- Recent apps --------------------------------------------------------------

/** Recently opened apps per (instance, account), for the home's 最近使ったアプリ. */
export const RECENT_APPS_STORAGE_KEY = "takosumi.mobile.recent-apps.v1";
export const RECENT_APPS_PER_SCOPE = 8;
export const MAX_RECENT_SCOPES = 16;

export interface RecentApp {
  readonly key: string;
  readonly at: string;
}

async function readRecentScopes(
  store: ChoiceStore | undefined,
): Promise<Map<string, readonly RecentApp[]>> {
  const scopes = new Map<string, readonly RecentApp[]>();
  const raw = await safeGet(store, RECENT_APPS_STORAGE_KEY);
  if (!raw) return scopes;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return scopes;
  }
  if (!isRecord(parsed) || !isRecord(parsed.scopes)) return scopes;
  for (const [scope, value] of Object.entries(parsed.scopes)) {
    if (!Array.isArray(value) || scope.length > APP_SCREEN_SCOPE_MAX_LENGTH) continue;
    const apps: RecentApp[] = [];
    for (const item of value) {
      if (!isRecord(item) || typeof item.key !== "string" || typeof item.at !== "string") {
        continue;
      }
      if (!item.key || item.key.length > 512 || apps.some((app) => app.key === item.key)) {
        continue;
      }
      apps.push({ key: item.key, at: item.at });
      if (apps.length >= RECENT_APPS_PER_SCOPE) break;
    }
    scopes.set(scope, apps);
  }
  return scopes;
}

export async function readRecentApps(
  store: ChoiceStore | undefined,
  scope: string,
): Promise<readonly RecentApp[]> {
  return (await readRecentScopes(store)).get(scope) ?? [];
}

export async function recordRecentApp(input: {
  readonly store: ChoiceStore | undefined;
  readonly scope: string;
  readonly key: string;
  readonly now?: () => Date;
}): Promise<readonly RecentApp[]> {
  if (!input.store) return [];
  const scopes = await readRecentScopes(input.store);
  const at = (input.now ?? (() => new Date()))().toISOString();
  const apps = [
    { key: input.key, at },
    ...(scopes.get(input.scope) ?? []).filter((app) => app.key !== input.key),
  ].slice(0, RECENT_APPS_PER_SCOPE);
  scopes.delete(input.scope);
  const kept = [[input.scope, apps] as const, ...scopes.entries()].slice(0, MAX_RECENT_SCOPES);
  await input.store.set(
    RECENT_APPS_STORAGE_KEY,
    JSON.stringify({ scopes: Object.fromEntries(kept) }),
  );
  return apps;
}

/** The recent apps that are still on the home, most recent first. */
export function recentLauncherEntries(
  recent: readonly RecentApp[],
  entries: readonly AppLauncherEntry[],
  limit = 4,
): readonly AppLauncherEntry[] {
  const byKey = new Map(entries.map((entry) => [entry.key, entry]));
  return recent.flatMap((app) => byKey.get(app.key) ?? []).slice(0, limit);
}

// --- Router ---------------------------------------------------------------------

export interface ChooserRequest {
  readonly target: OpenTarget;
  readonly reason: ChooserReason;
  readonly remembered?: OpenMode;
}

export type OpenOutcome =
  | { readonly kind: "opened"; readonly mode: OpenMode }
  | { readonly kind: "chooser" }
  | { readonly kind: "needs-confirmation" }
  | { readonly kind: "not-installed" };

export interface OpenRouter {
  /** A tile or a recent app: follow the remembered choice or the default. */
  readonly open: (target: OpenTarget) => Promise<OpenOutcome>;
  /**
   * Open in `mode` as chosen in the chooser. An unverified native client
   * opens only with `confirmedUnverified`; `remember` keeps the choice
   * (never `native` for an unverified client).
   */
  readonly openWith: (
    target: OpenTarget,
    mode: OpenMode,
    options?: { readonly remember?: boolean; readonly confirmedUnverified?: boolean },
  ) => Promise<OpenOutcome>;
  readonly choose: (target: OpenTarget, reason: ChooserReason) => Promise<void>;
  readonly forget: (target: OpenTarget) => Promise<void>;
  readonly remembered: (target: OpenTarget) => Promise<OpenMode | undefined>;
  /** An app screen bar's action: show the chooser for that app. */
  readonly handleAction: (
    action: AppScreenAction,
    entries: readonly AppLauncherEntry[],
  ) => Promise<void>;
  readonly recentApps: () => Promise<readonly RecentApp[]>;
}

export function createOpenRouter(input: {
  readonly opener: AppScreenOpener;
  /** Opens the app screen (icons, native request) — the launcher controller. */
  readonly openInApp: (target: OpenTarget, options: AppScreenOpenOptions) => Promise<void>;
  /** The phone's own browser (not the browser view), for 「ブラウザで開く」. */
  readonly openWeb: (url: string) => Promise<void>;
  readonly store?: ChoiceStore;
  readonly platform: string;
  readonly context: () => OpenContext | undefined;
  readonly showChooser: (request: ChooserRequest) => void;
  /** Called after recent apps change. */
  readonly onRecentChanged?: (recent: readonly RecentApp[]) => void;
  readonly now?: () => Date;
}): OpenRouter {
  const clientOf = (target: OpenTarget) => clientForPlatform(target.clients, input.platform);
  const choiceKey = (target: OpenTarget) => openChoiceKey(target, input.context());

  const recordRecent = async (target: OpenTarget) => {
    const context = input.context();
    if (!context) return;
    try {
      const recent = await recordRecentApp({
        store: input.store,
        scope: context.scope,
        key: target.key,
        ...(input.now ? { now: input.now } : {}),
      });
      input.onRecentChanged?.(recent);
    } catch {
      // Recent apps are a convenience; opening must not fail for them.
    }
  };

  const remembered = async (target: OpenTarget) => {
    try {
      return await readOpenChoice(input.store, choiceKey(target));
    } catch {
      return undefined;
    }
  };

  const choose = async (target: OpenTarget, reason: ChooserReason) => {
    const mode = await remembered(target);
    input.showChooser({ target, reason, ...(mode ? { remembered: mode } : {}) });
  };

  const openWith: OpenRouter["openWith"] = async (target, mode, options = {}) => {
    if (!isLaunchableAppScreenUrl(target.url)) {
      throw new Error("Only https URLs can be opened.");
    }
    const client = clientOf(target);
    if (mode === "native") {
      if (!client) throw new Error("この端末向けの専用アプリは宣言されていません。");
      if (!client.verified && !options.confirmedUnverified) {
        return { kind: "needs-confirmation" };
      }
    }
    if (options.remember) {
      try {
        await rememberOpenChoice({
          store: input.store,
          choiceKey: choiceKey(target),
          mode,
          ...(client ? { client } : {}),
          ...(input.now ? { now: input.now } : {}),
        });
      } catch {
        // Opening still goes ahead when the choice cannot be stored.
      }
    }
    switch (mode) {
      case "native": {
        const result = await input.opener.openClient(openClientRequest(client!));
        if (result === "not_installed") {
          await choose(target, "not-installed");
          return { kind: "not-installed" };
        }
        break;
      }
      case "web":
        await input.openWeb(target.url);
        break;
      case "browser-view":
        await input.opener.openBrowserView(target.url);
        break;
      case "in-app": {
        const context = input.context();
        await input.openInApp(target, {
          platform: input.platform,
          resume: true,
          ...(context ? { accountsOrigins: context.accountsOrigins, scope: context.scope } : {}),
        });
        break;
      }
    }
    await recordRecent(target);
    return { kind: "opened", mode };
  };

  return {
    async open(target) {
      const plan = planOpen({
        ...(clientOf(target) ? { client: clientOf(target)! } : {}),
        ...(target.clients ? { preferred: target.clients.preferred } : {}),
        ...(await remembered(target).then((mode) => (mode ? { remembered: mode } : {}))),
      });
      if (plan.kind === "choose") {
        await choose(target, plan.reason);
        return { kind: "chooser" };
      }
      return await openWith(target, plan.mode);
    },
    openWith,
    choose,
    async forget(target) {
      await forgetOpenChoice(input.store, choiceKey(target));
    },
    remembered,
    async handleAction(action, entries) {
      const entry = entries.find((candidate) => candidate.key === action.key);
      const target: OpenTarget = entry ?? resolveAppScreenTarget(action.url, entries);
      await choose(target, action.reason);
    },
    async recentApps() {
      const context = input.context();
      if (!context) return [];
      try {
        return await readRecentApps(input.store, context.scope);
      } catch {
        return [];
      }
    },
  };
}

async function safeGet(store: ChoiceStore | undefined, key: string): Promise<string | undefined> {
  if (!store) return undefined;
  try {
    return await store.get(key);
  } catch {
    return undefined;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
