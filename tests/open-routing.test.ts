import { expect, test } from "bun:test";
import type { MobileSession } from "@takosjp/mobile-kit";
import { decodeScreenClients, type ScreenClients } from "../src/app-clients.ts";
import type { AppScreenOpener } from "../src/app-screen-native.ts";
import type { AppLauncherEntry, AppScreenOpenOptions } from "../src/app-screens.ts";
import {
  type ChooserRequest,
  createOpenRouter,
  forgetOpenChoice,
  MAX_RECENT_SCOPES,
  OPEN_CHOICES_STORAGE_KEY,
  type OpenContext,
  type OpenTarget,
  openChoiceKey,
  openContextFromSession,
  planOpen,
  readOpenChoice,
  readRecentApps,
  RECENT_APPS_PER_SCOPE,
  recentLauncherEntries,
  recordRecentApp,
  rememberOpenChoice,
} from "../src/open-routing.ts";

function memoryStore(initial: Record<string, string> = {}) {
  const values = new Map(Object.entries(initial));
  return {
    kind: "device-persistent" as const,
    values,
    async get(key: string) {
      return values.get(key);
    },
    async set(key: string, value: string) {
      values.set(key, value);
    },
    async delete(key: string) {
      values.delete(key);
    },
  };
}

const verifiedClients = decodeScreenClients({
  preferred: "native",
  android: {
    verified: true,
    name: "Yurucommu",
    package: "com.yurucommu",
    open: "yurucommu://connect?host_url=https%3A%2F%2Fyc.example",
  },
  ios: {
    verified: true,
    name: "Yurucommu",
    universalLink: "https://open.yurucommu.example/connect#host_url=https%3A%2F%2Fyc.example",
    open: "yurucommu://connect?host_url=https%3A%2F%2Fyc.example",
  },
})!;
const unverifiedClients = decodeScreenClients({
  preferred: "web",
  android: { verified: false, package: "com.example.chat", open: "chatapp://connect" },
})!;
const webPreferred = decodeScreenClients({ ...verifiedClients, preferred: "web" })!;

function entry(key: string, clients?: ScreenClients): AppLauncherEntry {
  return {
    key,
    source: "takosumi",
    capsuleId: key.split(":")[1]!,
    interfaceId: "web",
    label: key,
    url: "https://yc.example/",
    host: "yc.example",
    fallbackGlyph: "Y",
    ...(clients ? { clients } : {}),
  };
}

const context: OpenContext = {
  instanceOrigin: "https://takosumi.example",
  accountsOrigin: "https://accounts.takosumi.example",
  scope: "https://takosumi.example|sub_1",
};

function harness(input: {
  readonly platform?: string;
  readonly clientResult?: "opened" | "not_installed";
  readonly store?: ReturnType<typeof memoryStore>;
} = {}) {
  const calls: Array<{ kind: string; value: unknown }> = [];
  const choosers: ChooserRequest[] = [];
  const store = input.store ?? memoryStore();
  const opener: AppScreenOpener = {
    capabilities: async () => ({ inApp: true, pinShortcut: false }),
    open: async () => undefined,
    pinShortcut: async () => false,
    openClient: async (request) => {
      calls.push({ kind: "client", value: request });
      return input.clientResult ?? "opened";
    },
    openBrowserView: async (url) => {
      calls.push({ kind: "browser-view", value: url });
      return "browser-view";
    },
    takePendingAction: async () => undefined,
    onActionPending: async () => () => undefined,
  };
  const router = createOpenRouter({
    opener,
    openInApp: async (target: OpenTarget, options: AppScreenOpenOptions) =>
      void calls.push({ kind: "in-app", value: { key: target.key, ...options } }),
    openWeb: async (url) => void calls.push({ kind: "web", value: url }),
    store,
    platform: input.platform ?? "android",
    context: () => context,
    showChooser: (request) => void choosers.push(request),
    now: () => new Date("2026-10-30T00:00:00.000Z"),
  });
  return { router, calls, choosers, store };
}

test("the plan follows the remembered choice, then a verified declaration, then 中で見る", () => {
  const verified = verifiedClients.android!;
  const unverified = unverifiedClients.android!;
  expect(planOpen({})).toEqual({ kind: "open", mode: "in-app" });
  expect(planOpen({ client: verified, preferred: "native" })).toEqual({ kind: "open", mode: "native" });
  // `preferred: "web"` is not native; Takosumi Mobile's web default is 中で見る.
  expect(planOpen({ client: verified, preferred: "web" })).toEqual({ kind: "open", mode: "in-app" });
  expect(planOpen({ client: unverified, preferred: "web" })).toEqual({ kind: "choose", reason: "unverified" });
  expect(planOpen({ client: verified, preferred: "native", remembered: "web" })).toEqual({
    kind: "open",
    mode: "web",
  });
  expect(planOpen({ client: unverified, remembered: "browser-view" })).toEqual({
    kind: "open",
    mode: "browser-view",
  });
  // A remembered native choice never opens an unverified client by itself.
  expect(planOpen({ client: unverified, remembered: "native" })).toEqual({
    kind: "choose",
    reason: "unverified",
  });
  expect(planOpen({ remembered: "native" })).toEqual({ kind: "open", mode: "in-app" });
});

test("a tile without a declaration opens inside Takosumi at its last page, with the Accounts origin", async () => {
  const { router, calls } = harness();
  expect(await router.open(entry("app:cap_a:web"))).toEqual({ kind: "opened", mode: "in-app" });
  expect(calls).toEqual([
    {
      kind: "in-app",
      value: {
        key: "app:cap_a:web",
        platform: "android",
        resume: true,
        accountsOrigin: "https://accounts.takosumi.example",
        scope: "https://takosumi.example|sub_1",
      },
    },
  ]);
  expect(await router.recentApps()).toEqual([
    { key: "app:cap_a:web", at: "2026-10-30T00:00:00.000Z" },
  ]);
});

test("a verified native default opens the app; when missing, the chooser comes up and the choice stays", async () => {
  const opened = harness();
  expect(await opened.router.open(entry("app:cap_y:web", verifiedClients))).toEqual({
    kind: "opened",
    mode: "native",
  });
  expect(opened.calls[0]).toEqual({
    kind: "client",
    value: {
      platform: "android",
      package: "com.yurucommu",
      open: "yurucommu://connect?host_url=https%3A%2F%2Fyc.example",
    },
  });
  expect(opened.choosers).toEqual([]);

  const missing = harness({ clientResult: "not_installed" });
  const target = entry("app:cap_y:web", verifiedClients);
  await rememberOpenChoice({
    store: missing.store,
    choiceKey: openChoiceKey(target, context),
    mode: "native",
    client: verifiedClients.android!,
  });
  expect(await missing.router.open(target)).toEqual({ kind: "not-installed" });
  expect(missing.choosers).toEqual([{ target, reason: "not-installed", remembered: "native" }]);
  expect(await readOpenChoice(missing.store, openChoiceKey(target, context))).toBe("native");

  const ios = harness({ platform: "ios" });
  await ios.router.open(entry("app:cap_y:web", verifiedClients));
  expect(ios.calls[0]?.value).toEqual({
    platform: "ios",
    universalLink: "https://open.yurucommu.example/connect#host_url=https%3A%2F%2Fyc.example",
    open: "yurucommu://connect?host_url=https%3A%2F%2Fyc.example",
  });
});

test("an unverified native app is never opened without confirmation and never remembered", async () => {
  const { router, calls, choosers, store } = harness();
  const target = entry("app:cap_u:web", unverifiedClients);
  expect(await router.open(target)).toEqual({ kind: "chooser" });
  expect(choosers).toEqual([{ target, reason: "unverified" }]);
  expect(calls).toEqual([]);

  expect(await router.openWith(target, "native", { remember: true })).toEqual({
    kind: "needs-confirmation",
  });
  expect(calls).toEqual([]);
  expect(await router.openWith(target, "native", { remember: true, confirmedUnverified: true }))
    .toEqual({ kind: "opened", mode: "native" });
  expect(calls).toEqual([
    { kind: "client", value: { platform: "android", package: "com.example.chat", open: "chatapp://connect" } },
  ]);
  expect(await readOpenChoice(store, openChoiceKey(target, context))).toBeUndefined();
  // Even a hand-written store entry does not open it next time.
  await rememberOpenChoice({ store, choiceKey: openChoiceKey(target, context), mode: "native" });
  expect(await readOpenChoice(store, openChoiceKey(target, context))).toBeUndefined();
  store.values.set(
    OPEN_CHOICES_STORAGE_KEY,
    JSON.stringify({ choices: { [openChoiceKey(target, context)]: { mode: "native", at: "x" } } }),
  );
  expect(await router.open(target)).toEqual({ kind: "chooser" });
});

test("chosen web and browser-view open the right surface and can be remembered and forgotten", async () => {
  const { router, calls, store } = harness();
  const target = entry("app:cap_w:web", webPreferred);
  await router.openWith(target, "browser-view", { remember: true });
  expect(calls.at(-1)).toEqual({ kind: "browser-view", value: "https://yc.example/" });
  expect(await router.remembered(target)).toBe("browser-view");
  await router.open(target);
  expect(calls.at(-1)).toEqual({ kind: "browser-view", value: "https://yc.example/" });
  await router.openWith(target, "web", { remember: true });
  expect(calls.at(-1)).toEqual({ kind: "web", value: "https://yc.example/" });
  await router.forget(target);
  expect(await router.remembered(target)).toBeUndefined();
  await router.open(target);
  expect(calls.at(-1)?.kind).toBe("in-app");
  await forgetOpenChoice(store, "missing");
});

test("choices are kept per instance for Takosumi screens and per device for added URLs", () => {
  expect(openChoiceKey({ key: "app:cap_a:web", source: "takosumi" }, context)).toBe(
    "https://takosumi.example\napp:cap_a:web",
  );
  expect(openChoiceKey({ key: "custom:abc", source: "custom" }, context)).toBe("device\ncustom:abc");
  expect(openChoiceKey({ key: "origin:https://x.example" }, context)).toBe(
    "device\norigin:https://x.example",
  );
});

test("an app screen's bar action brings up the chooser for that app", async () => {
  const { router, choosers } = harness();
  const known = entry("app:cap_y:web", verifiedClients);
  await router.handleAction(
    { kind: "choose", key: "app:cap_y:web", url: "https://yc.example/", reason: "change" },
    [known],
  );
  expect(choosers.at(-1)).toEqual({ target: known, reason: "change" });
  await router.handleAction(
    { kind: "choose", key: "origin:https://x.example", url: "https://x.example/a", reason: "change" },
    [known],
  );
  expect(choosers.at(-1)?.target.key).toBe("origin:https://x.example");
});

test("recent apps are kept per (instance, account), newest first and bounded", async () => {
  const store = memoryStore();
  let tick = 0;
  const now = () => new Date(Date.UTC(2026, 9, 30, 0, 0, tick++));
  for (let index = 0; index < RECENT_APPS_PER_SCOPE + 3; index++) {
    await recordRecentApp({ store, scope: "s1", key: `app:cap_${index}:web`, now });
  }
  await recordRecentApp({ store, scope: "s1", key: "app:cap_2:web", now });
  const recent = await readRecentApps(store, "s1");
  expect(recent.length).toBe(RECENT_APPS_PER_SCOPE);
  expect(recent[0]?.key).toBe("app:cap_2:web");
  expect(recent.filter((app) => app.key === "app:cap_2:web").length).toBe(1);
  expect(await readRecentApps(store, "s2")).toEqual([]);
  for (let index = 0; index < MAX_RECENT_SCOPES + 2; index++) {
    await recordRecentApp({ store, scope: `scope-${index}`, key: "app:cap_a:web", now });
  }
  expect(await readRecentApps(store, "s1")).toEqual([]);
  expect((await readRecentApps(store, `scope-${MAX_RECENT_SCOPES + 1}`)).length).toBe(1);

  const shown = recentLauncherEntries(
    [{ key: "app:gone:web", at: "x" }, { key: "app:cap_b:web", at: "y" }],
    [entry("app:cap_a:web"), entry("app:cap_b:web")],
  );
  expect(shown.map((item) => item.key)).toEqual(["app:cap_b:web"]);
  store.values.set("takosumi.mobile.recent-apps.v1", "{not json");
  expect(await readRecentApps(store, "s1")).toEqual([]);
});

test("the open context follows the session's instance and account", () => {
  const session: MobileSession = {
    hostUrl: "https://takosumi.example/",
    oidcIssuer: "https://accounts.takosumi.example",
    accountId: "sub_1",
    product: "takosumi",
    accessToken: "takat_fixture",
    tokenType: "Bearer",
    createdAt: "2026-10-30T00:00:00.000Z",
  };
  expect(openContextFromSession(session)).toEqual(context);
  expect(openContextFromSession({ ...session, accountId: undefined } as MobileSession)?.scope).toBe(
    "https://takosumi.example|",
  );
});
