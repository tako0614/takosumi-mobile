import { afterEach, expect, test } from "bun:test";
import type { MobileSession, NativeBridge } from "@takosjp/mobile-kit";
import { loadAppScreens } from "../src/api.ts";
import {
  fetchIconDataUrl,
  ICON_MAX_BYTES,
  nativeIconSource,
  sniffIconFormat,
} from "../src/app-icons.ts";
import {
  APP_SCREEN_BAR_COLOR,
  type AppScreenInvoke,
  createAppScreenOpener,
  decodeAction,
} from "../src/app-screen-native.ts";
import type { AppLauncherEntry, AppScreenOpenRequest } from "../src/app-screens.ts";
import {
  addCustomApp,
  CUSTOM_APPS_STORAGE_KEY,
  MAX_CUSTOM_APPS,
  readCustomApps,
  removeCustomApp,
} from "../src/custom-apps.ts";
import { withAppScreenDeepLinks } from "../src/deep-links.ts";
import { launcherAppsFromHome } from "../src/home-presentation.ts";
import { createLauncherController } from "../src/launcher-controller.ts";

const originalFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = originalFetch;
});

const session: MobileSession = {
  hostUrl: "https://operator.example",
  product: "takosumi",
  accessToken: "takat_fixture_mobile_access",
  tokenType: "Bearer",
  scope: "capsules:read runs:read",
  createdAt: "2026-10-29T00:00:00.000Z",
};

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

// --- Screens loading -------------------------------------------------------

test("screens load per app; absent views and denied apps are skipped, failures reported", async () => {
  const paths: string[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input));
    paths.push(url.pathname);
    expect(new Headers(init?.headers).get("authorization")).toBe(
      "Bearer takat_fixture_mobile_access",
    );
    switch (url.pathname) {
      case "/api/v1/capsules/cap_ok/app":
        return Response.json({
          app: {
            kind: "takosumi.capsule-app@v1",
            capsuleId: "cap_ok",
            screens: [{ interfaceId: "web", name: "web", url: "https://ok.example/" }],
          },
        });
      case "/api/v1/capsules/cap_old_host/app":
        return new Response(null, { status: 404 });
      case "/api/v1/capsules/cap_denied/app":
        return Response.json({ error: { code: "forbidden" } }, { status: 403 });
      case "/api/v1/capsules/cap_broken/app":
        return Response.json({ message: "unavailable" }, { status: 503 });
      case "/api/v1/capsules/cap%2Fslash/app":
        return Response.json({ app: { capsuleId: "cap_other", screens: [] } });
      default:
        throw new Error(`Unexpected request: ${url.pathname}`);
    }
  }) as typeof fetch;

  const result = await loadAppScreens(session, [
    { capsuleId: "cap_ok", label: "OK", state: "installed" },
    { capsuleId: "cap_old_host", label: "Old", state: "active" },
    { capsuleId: "cap_denied", label: "Denied", state: "installed" },
    { capsuleId: "cap_broken", label: "Broken", state: "needs_attention" },
    { capsuleId: "cap/slash", label: "Mismatch", state: "installed" },
    { capsuleId: "cap_installing", label: "New", state: "installing" },
    { capsuleId: "cap_gone", label: "Gone", state: "uninstalled" },
  ]);
  expect([...result.screens.keys()]).toEqual(["cap_ok"]);
  expect(result.screens.get("cap_ok")?.screens[0]?.url).toBe("https://ok.example/");
  expect([...result.failedCapsuleIds].sort()).toEqual(["cap/slash", "cap_broken"]);
  expect(paths.sort()).toEqual([
    "/api/v1/capsules/cap%2Fslash/app",
    "/api/v1/capsules/cap_broken/app",
    "/api/v1/capsules/cap_denied/app",
    "/api/v1/capsules/cap_ok/app",
    "/api/v1/capsules/cap_old_host/app",
  ]);
});

test("the launcher reads the per-app list, or the legacy Workspace Capsules", () => {
  expect(
    launcherAppsFromHome(
      {
        apps: [
          {
            capsuleId: "cap_a",
            name: "takos-office",
            displayName: { ja: "オフィス", en: "Office" },
            iconUrl: "https://store.example/o.png",
            role: "owner",
            ownerDisplayName: "Me",
            state: "installed",
            primaryAction: "open",
          },
        ],
        workspaces: [],
        capsules: [],
        runs: [],
      },
      "ja-JP",
    ),
  ).toEqual([
    { capsuleId: "cap_a", label: "オフィス", iconUrl: "https://store.example/o.png", state: "installed" },
  ]);
  expect(
    launcherAppsFromHome({
      apps: [],
      workspaces: [],
      capsules: [
        {
          id: "cap_legacy",
          name: "yurucommu",
          status: "active",
        } as never,
      ],
      runs: [],
    }),
  ).toEqual([{ capsuleId: "cap_legacy", label: "yurucommu", state: "active" }]);
  expect(launcherAppsFromHome(undefined)).toEqual([]);
});

// --- Device-only URLs ------------------------------------------------------

test("added URLs are stored only on the device, deduplicated and https-only", async () => {
  const store = memoryStore();
  let id = 0;
  const createId = () => `entry-${String(++id).padStart(4, "0")}`;
  const now = () => new Date("2026-10-29T01:02:03.000Z");

  const first = await addCustomApp({ store, url: "news.example", title: " News ", createId, now });
  expect(first).toEqual({
    ok: true,
    existed: false,
    entry: { id: "entry-0001", url: "https://news.example/", title: "News", addedAt: "2026-10-29T01:02:03.000Z" },
    entries: [{ id: "entry-0001", url: "https://news.example/", title: "News", addedAt: "2026-10-29T01:02:03.000Z" }],
  });
  const again = await addCustomApp({ store, url: "https://news.example/#top", createId, now });
  expect(again.ok && again.existed).toBe(true);
  expect(await addCustomApp({ store, url: "http://news.example", createId, now })).toEqual({
    ok: false,
    error: "https のURLだけ追加できます。",
  });
  const untitled = await addCustomApp({ store, url: "https://blog.example/feed", createId, now });
  expect(untitled.ok && untitled.entry.title).toBe("blog.example");
  expect((await readCustomApps(store)).map((entry) => entry.id)).toEqual([
    "entry-0001",
    "entry-0002",
  ]);
  expect(JSON.parse(store.values.get(CUSTOM_APPS_STORAGE_KEY)!)).toHaveLength(2);

  expect((await removeCustomApp(store, "entry-0001")).map((entry) => entry.id)).toEqual([
    "entry-0002",
  ]);
  expect(await addCustomApp({ store: undefined, url: "https://x.example" })).toEqual({
    ok: false,
    error: "この端末では追加したURLを保存できません。",
  });
});

test("stored URLs are re-validated when read and capped", async () => {
  const store = memoryStore({
    [CUSTOM_APPS_STORAGE_KEY]: JSON.stringify([
      { id: "good-0001", url: "https://good.example/", title: "Good", addedAt: "2026-10-29T00:00:00.000Z" },
      { id: "bad-url-01", url: "http://bad.example/", title: "Bad", addedAt: "2026-10-29T00:00:00.000Z" },
      { id: "x", url: "https://short-id.example/", title: "Short id" },
      { id: "good-0001", url: "https://dup.example/", title: "Duplicate id" },
      { id: "no-title-1", url: "https://untitled.example/" },
      "junk",
    ]),
  });
  expect(await readCustomApps(store)).toEqual([
    { id: "good-0001", url: "https://good.example/", title: "Good", addedAt: "2026-10-29T00:00:00.000Z" },
    { id: "no-title-1", url: "https://untitled.example/", title: "untitled.example", addedAt: "1970-01-01T00:00:00.000Z" },
  ]);
  expect(await readCustomApps(memoryStore({ [CUSTOM_APPS_STORAGE_KEY]: "{not json" }))).toEqual([]);

  const full = memoryStore({
    [CUSTOM_APPS_STORAGE_KEY]: JSON.stringify(
      Array.from({ length: MAX_CUSTOM_APPS }, (_, index) => ({
        id: `full-${String(index).padStart(4, "0")}`,
        url: `https://site${index}.example/`,
        title: `Site ${index}`,
        addedAt: "2026-10-29T00:00:00.000Z",
      })),
    ),
  });
  expect(await addCustomApp({ store: full, url: "https://one-more.example" })).toEqual({
    ok: false,
    error: `追加できるURLは${MAX_CUSTOM_APPS}件までです。`,
  });
});

// --- Icons -------------------------------------------------------------------

const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13]);
const SVG = new TextEncoder().encode('<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg"></svg>');

test("icon formats are sniffed from the bytes, SVG only with an SVG or XML type", () => {
  expect(sniffIconFormat("image/png", PNG)).toBe("png");
  expect(sniffIconFormat("application/octet-stream", PNG)).toBe("png");
  expect(sniffIconFormat("image/svg+xml", SVG)).toBe("svg");
  expect(sniffIconFormat("text/html", SVG)).toBeUndefined();
  expect(sniffIconFormat("image/png", new TextEncoder().encode("<html>"))).toBeUndefined();
  expect(sniffIconFormat(null, new Uint8Array([0xff, 0xd8, 0xff, 0xe0]))).toBe("jpeg");
  expect(
    sniffIconFormat("image/webp", new TextEncoder().encode("RIFF\u0000\u0000\u0000\u0000WEBPVP8 ")),
  ).toBe("webp");
});

test("icons are fetched without credentials, https-only and size-limited", async () => {
  const requests: Array<{ url: string; init?: RequestInit }> = [];
  const fetchImpl = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    requests.push({ url, init });
    if (url.endsWith("/icon.svg")) {
      return new Response(SVG, { headers: { "content-type": "image/svg+xml" } });
    }
    if (url.endsWith("/huge.png")) {
      return new Response(new Uint8Array(ICON_MAX_BYTES + 1), {
        headers: { "content-type": "image/png" },
      });
    }
    if (url.endsWith("/page.html")) {
      return new Response("<html></html>", { headers: { "content-type": "text/html" } });
    }
    if (url.endsWith("/missing.png")) return new Response(null, { status: 404 });
    throw new Error("network");
  }) as typeof fetch;

  const svg = await fetchIconDataUrl("https://app.example/icon.svg", fetchImpl);
  expect(svg?.startsWith("data:image/svg+xml;base64,")).toBe(true);
  expect(requests[0]?.init?.credentials).toBe("omit");
  expect(await fetchIconDataUrl("https://app.example/huge.png", fetchImpl)).toBeUndefined();
  expect(await fetchIconDataUrl("https://app.example/page.html", fetchImpl)).toBeUndefined();
  expect(await fetchIconDataUrl("https://app.example/missing.png", fetchImpl)).toBeUndefined();
  expect(await fetchIconDataUrl("https://app.example/offline.png", fetchImpl)).toBeUndefined();
  const before = requests.length;
  expect(await fetchIconDataUrl("http://app.example/icon.svg", fetchImpl)).toBeUndefined();
  expect(await fetchIconDataUrl("https://u:p@app.example/icon.svg", fetchImpl)).toBeUndefined();
  expect(requests.length).toBe(before);
});

test("native icons use the loaded image, else the glyph or the first letter", () => {
  expect(nativeIconSource({ kind: "image", url: "https://x.example/i.png" }, "data:image/png;base64,AA", "X")).toEqual({
    kind: "data-url",
    dataUrl: "data:image/png;base64,AA",
  });
  expect(nativeIconSource({ kind: "image", url: "https://x.example/i.png" }, undefined, "X")).toEqual({
    kind: "glyph",
    glyph: "X",
  });
  expect(nativeIconSource({ kind: "glyph", glyph: "🐙" }, undefined, "X")).toEqual({
    kind: "glyph",
    glyph: "🐙",
  });
});

// --- Native opener ------------------------------------------------------------

const request: AppScreenOpenRequest = {
  key: "app:cap_a:web",
  url: "https://a.example/",
  title: "A",
  profile: "shared",
};

test("Android and iOS open screens through the app-screen plugin", async () => {
  const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];
  const invoke = (async (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, ...(args ? { args } : {}) });
    if (command === "plugin:app-screen|capabilities") return { pinShortcut: true };
    if (command === "plugin:app-screen|pin_shortcut") return { requested: true };
    return null;
  }) as AppScreenInvoke;
  const external: string[] = [];
  const android = createAppScreenOpener({
    invoke,
    platform: "android",
    openExternalUrl: async (url) => void external.push(url),
  });
  expect(await android.capabilities()).toEqual({ inApp: true, pinShortcut: true });
  await android.open(request);
  expect(await android.pinShortcut(request)).toBe(true);
  expect(calls).toEqual([
    { command: "plugin:app-screen|capabilities" },
    { command: "plugin:app-screen|open", args: { payload: request } },
    { command: "plugin:app-screen|pin_shortcut", args: { payload: request } },
  ]);
  expect(external).toEqual([]);

  const ios = createAppScreenOpener({ invoke, platform: "ios", openExternalUrl: async () => {} });
  expect(await ios.capabilities()).toEqual({ inApp: true, pinShortcut: false });
  expect(await ios.pinShortcut(request)).toBe(false);

  await expect(android.open({ ...request, url: "http://a.example/" })).rejects.toThrow("https");
  await expect(
    android.open({ ...request, profile: "per-app" as unknown as "shared" }),
  ).rejects.toThrow("one browsing profile");
});

test("native apps, the browser view and bar actions go through the app-screen plugin", async () => {
  const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];
  let pending: unknown = {
    kind: "choose",
    key: "app:cap_a:web",
    url: "https://a.example/inbox",
    reason: "not-installed",
  };
  const invoke = (async (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, ...(args ? { args } : {}) });
    if (command === "plugin:app-screen|open_client") return { result: "not_installed" };
    if (command === "plugin:app-screen|open_browser_view") return { result: "browser" };
    if (command === "plugin:app-screen|take_pending_action") {
      const action = pending;
      pending = undefined;
      return action ? { action } : {};
    }
    return null;
  }) as AppScreenInvoke;
  const listened: string[] = [];
  const android = createAppScreenOpener({
    invoke,
    listen: async (plugin, event, handler) => {
      listened.push(`${plugin}:${event}`);
      handler(undefined);
      return { unregister: async () => undefined };
    },
    platform: "android",
    openExternalUrl: async () => {},
  });
  const client = { platform: "android" as const, package: "com.yurucommu", open: "yurucommu://x" };
  expect(await android.openClient(client)).toBe("not_installed");
  // A request for the other phone is never sent.
  expect(await android.openClient({ platform: "ios", open: "yurucommu://x" })).toBe("not_installed");
  expect(await android.openBrowserView("https://accounts.google.com/")).toBe("browser");
  await android.openWeb("https://a.example/");
  await expect(android.openWeb("http://a.example/")).rejects.toThrow("https");
  await expect(android.openBrowserView("http://a.example/")).rejects.toThrow("https");
  expect(await android.takePendingAction()).toEqual({
    kind: "choose",
    key: "app:cap_a:web",
    url: "https://a.example/inbox",
    reason: "not-installed",
  });
  expect(await android.takePendingAction()).toBeUndefined();
  let nudged = 0;
  await android.onActionPending(() => void nudged++);
  expect(nudged).toBe(1);
  expect(listened).toEqual(["app-screen:actionPending"]);
  expect(calls.slice(0, 3)).toEqual([
    { command: "plugin:app-screen|open_client", args: { payload: client } },
    {
      command: "plugin:app-screen|open_browser_view",
      args: { payload: { url: "https://accounts.google.com/", toolbarColor: APP_SCREEN_BAR_COLOR } },
    },
    { command: "plugin:app-screen|open_web", args: { payload: { url: "https://a.example/" } } },
  ]);
});

test("bar actions are taken only in their known shape", () => {
  expect(decodeAction({ kind: "choose", key: "k", url: "https://a.example/", reason: "x" })).toEqual({
    kind: "choose",
    key: "k",
    url: "https://a.example/",
    reason: "change",
  });
  expect(decodeAction({ kind: "open", key: "k", url: "https://a.example/" })).toBeUndefined();
  expect(decodeAction({ kind: "choose", key: "k", url: "javascript:alert(1)" })).toBeUndefined();
  expect(decodeAction({ kind: "choose", key: "", url: "https://a.example/" })).toBeUndefined();
  expect(decodeAction(null)).toBeUndefined();
});

test("the web preview opens screens in the external browser", async () => {
  const external: string[] = [];
  const web = createAppScreenOpener({
    platform: "web",
    openExternalUrl: async (url) => void external.push(url),
  });
  expect(await web.capabilities()).toEqual({ inApp: false, pinShortcut: false });
  await web.open(request);
  expect(await web.pinShortcut(request)).toBe(false);
  expect(await web.openClient({ platform: "android", package: "com.a", open: "a://x" })).toBe(
    "not_installed",
  );
  expect(await web.openBrowserView("https://b.example/")).toBe("browser");
  expect(await web.takePendingAction()).toBeUndefined();
  expect(external).toEqual(["https://a.example/", "https://b.example/"]);
});

// --- Launcher controller ------------------------------------------------------

const yurucommu: AppLauncherEntry = {
  key: "app:cap_yuru:web",
  source: "takosumi",
  capsuleId: "cap_yuru",
  interfaceId: "web",
  label: "Yurucommu",
  url: "https://yuru.example/",
  host: "yuru.example",
  icon: { kind: "image", url: "https://yuru.example/icon.svg" },
  fallbackGlyph: "Y",
};

function recordingOpener() {
  const opened: AppScreenOpenRequest[] = [];
  const pinned: AppScreenOpenRequest[] = [];
  return {
    opened,
    pinned,
    opener: {
      capabilities: async () => ({ inApp: true, pinShortcut: true }),
      open: async (value: AppScreenOpenRequest) => void opened.push(value),
      pinShortcut: async (value: AppScreenOpenRequest) => {
        pinned.push(value);
        return true;
      },
      openClient: async () => "not_installed" as const,
      openBrowserView: async () => "browser-view" as const,
      openWeb: async () => undefined,
      takePendingAction: async () => undefined,
      onActionPending: async () => () => undefined,
    },
  };
}

test("opening an entry passes its rasterized icon and caches the icon fetch", async () => {
  const { opener, opened, pinned } = recordingOpener();
  const fetched: string[] = [];
  const controller = createLauncherController({
    opener,
    fetchIcon: async (url) => {
      fetched.push(url);
      return "data:image/svg+xml;base64,PHN2Zy8+";
    },
    rasterize: async (source) => (source.kind === "data-url" ? "UE5HLWltYWdl" : "UE5HLWdseXBo"),
  });
  controller.setEntries([yurucommu]);
  await controller.openTarget(yurucommu);
  await controller.pinEntry(yurucommu);
  expect(await controller.iconDataUrl(yurucommu.icon)).toBe("data:image/svg+xml;base64,PHN2Zy8+");
  expect(fetched).toEqual(["https://yuru.example/icon.svg"]);
  expect(opened).toEqual([
    { key: "app:cap_yuru:web", url: "https://yuru.example/", title: "Yurucommu", iconPng: "UE5HLWltYWdl", profile: "shared" },
  ]);
  expect(pinned[0]?.iconPng).toBe("UE5HLWltYWdl");
});

test("deep-linked URLs open in the matching app's screen; slow icons do not block", async () => {
  const { opener, opened } = recordingOpener();
  const controller = createLauncherController({
    opener,
    fetchIcon: () => new Promise(() => {}),
    rasterize: async (source) => (source.kind === "glyph" ? `glyph-${source.glyph}` : "image"),
    iconWaitMs: 5,
  });
  controller.setEntries([yurucommu]);
  await controller.openUrl("https://yuru.example/notifications");
  await controller.openUrl("https://elsewhere.example/");
  expect(opened).toEqual([
    // The image icon never arrives; the screen opens without one after the wait.
    { key: "app:cap_yuru:web", url: "https://yuru.example/notifications", title: "Yurucommu", profile: "shared" },
    // An unknown origin is named by its host and drawn from its first letter.
    {
      key: "origin:https://elsewhere.example",
      url: "https://elsewhere.example/",
      title: "elsewhere.example",
      iconPng: "glyph-E",
      profile: "shared",
    },
  ]);
});

test("a Takosumi page outside sign-in opens in the browser view, never in an app screen", async () => {
  const { opener, opened } = recordingOpener();
  const browserViews: string[] = [];
  const controller = createLauncherController({
    opener: {
      ...opener,
      openBrowserView: async (url: string) => {
        browserViews.push(url);
        return "browser-view" as const;
      },
    },
    fetchIcon: async () => undefined,
    rasterize: async () => undefined,
    iconWaitMs: 5,
  });
  const accountsOrigins = ["https://app.takosumi.example"];
  await controller.openUrl("https://app.takosumi.example/apps", { accountsOrigins });
  await controller.openUrl("https://app.takosumi.example/oauth/authorize?client_id=x", {
    accountsOrigins,
  });
  expect(browserViews).toEqual(["https://app.takosumi.example/apps"]);
  expect(opened.map((request) => request.url)).toEqual([
    "https://app.takosumi.example/oauth/authorize?client_id=x",
  ]);
  await expect(
    controller.pinEntry(
      { ...yurucommu, key: "custom:dash", url: "https://app.takosumi.example/" },
      { accountsOrigins },
    ),
  ).rejects.toThrow("ホーム画面に追加できません");
});

// --- Deep links -----------------------------------------------------------------

test("app screen links are taken before the shared shell; other payloads pass", async () => {
  let listener: ((payload: string) => void) | undefined;
  const base = {
    capabilities: {} as NativeBridge["capabilities"],
    getLaunchPayload: async () => "takosumi://app?url=https%3A%2F%2Fyuru.example%2F",
    onLaunchPayload: async (handler: (payload: string) => void) => {
      listener = handler;
      return () => {};
    },
    openExternalUrl: async () => {},
  } satisfies NativeBridge;
  const links: unknown[] = [];
  const shell: string[] = [];
  const bridge = withAppScreenDeepLinks(base, (link) => links.push(link));

  expect(await bridge.getLaunchPayload()).toBeUndefined();
  // The launch URL is read again on later starts; it opens only once.
  expect(await bridge.getLaunchPayload()).toBeUndefined();
  await bridge.onLaunchPayload!((payload) => shell.push(payload));
  listener!("takosumi://oauth/callback?code=c&state=s");
  listener!("takosumi://app?url=https%3A%2F%2Fyuru.example%2Fnotifications");
  listener!("takosumi://app?url=http%3A%2F%2Finsecure.example");
  listener!("https://app.takosumi.com");

  expect(links).toEqual([
    { ok: true, url: "https://yuru.example/" },
    { ok: true, url: "https://yuru.example/notifications" },
    { ok: false, error: "https のURLだけ開けます。" },
  ]);
  expect(shell).toEqual(["takosumi://oauth/callback?code=c&state=s", "https://app.takosumi.com"]);

  const passthrough = withAppScreenDeepLinks(
    { ...base, getLaunchPayload: async () => "takosumi://oauth/callback?code=c" },
    () => {
      throw new Error("not an app screen link");
    },
  );
  expect(await passthrough.getLaunchPayload()).toBe("takosumi://oauth/callback?code=c");
});
