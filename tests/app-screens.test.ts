import { expect, test } from "bun:test";
import {
  APP_SCREEN_PROFILE,
  appScreenDeepLink,
  appScreenOpenRequest,
  buildLauncherEntries,
  CAPSULE_APP_SCREENS_DECODER,
  type CapsuleAppScreens,
  decodeIcon,
  parseAppScreenDeepLink,
  resolveAppScreenTarget,
  stabilizeLauncherEntries,
} from "../src/app-screens.ts";

function view(app: Record<string, unknown>) {
  return CAPSULE_APP_SCREENS_DECODER.decode({
    app: { kind: "takosumi.capsule-app@v1", capsuleId: "cap_a", ...app },
  });
}

test("the app view decoder keeps launchable https screens with their display icon", () => {
  const decoded = view({
    screens: [
      {
        interfaceId: "if_web",
        name: "web",
        title: "Yurucommu",
        icon: { kind: "image", url: "https://yurucommu.example/icon.svg" },
        sortOrder: 1,
        url: "https://yurucommu.example/",
      },
      { interfaceId: "if_glyph", name: "admin", icon: { kind: "glyph", glyph: "🛠" }, url: "https://yurucommu.example/admin" },
      { interfaceId: "if_http", name: "dev", url: "http://localhost:8787/" },
      { interfaceId: "if_bad", name: "bad", url: "javascript:alert(1)" },
      { name: "missing-id", url: "https://x.example/" },
      "not-an-object",
    ],
  });
  expect(decoded).toEqual({
    capsuleId: "cap_a",
    complete: true,
    screens: [
      {
        interfaceId: "if_web",
        name: "web",
        title: "Yurucommu",
        icon: { kind: "image", url: "https://yurucommu.example/icon.svg" },
        sortOrder: 1,
        url: "https://yurucommu.example/",
      },
      {
        interfaceId: "if_glyph",
        name: "admin",
        icon: { kind: "glyph", glyph: "🛠" },
        url: "https://yurucommu.example/admin",
      },
    ],
  });
});

test("the decoder reports incomplete screens and rejects a different view kind", () => {
  expect(view({ screens: [], incomplete: ["screens"] }).complete).toBe(false);
  expect(view({}).complete).toBe(false);
  expect(view({ screens: [], incomplete: ["links"] }).complete).toBe(true);
  expect(() =>
    CAPSULE_APP_SCREENS_DECODER.decode({ app: { kind: "other@v1", capsuleId: "c" } }),
  ).toThrow("app.kind");
  expect(() => CAPSULE_APP_SCREENS_DECODER.decode({ app: {} })).toThrow("app.capsuleId");
});

test("icons accept only credential-free https images and short glyphs", () => {
  expect(decodeIcon({ kind: "image", url: "http://x.example/i.png" })).toBeUndefined();
  expect(decodeIcon({ kind: "image", url: "https://u:p@x.example/i.png" })).toBeUndefined();
  expect(decodeIcon({ kind: "image", url: "/relative.png" })).toBeUndefined();
  expect(decodeIcon({ kind: "glyph", glyph: "x".repeat(17) })).toBeUndefined();
  expect(decodeIcon({ kind: "glyph", glyph: "🐙" })).toEqual({ kind: "glyph", glyph: "🐙" });
  expect(decodeIcon("https://x.example/i.png")).toBeUndefined();
});

function screensFor(
  capsuleId: string,
  screens: CapsuleAppScreens["screens"],
): [string, CapsuleAppScreens] {
  return [capsuleId, { capsuleId, screens, complete: true }];
}

test("the home lists each app's screens as icons, then the URLs added on the device", () => {
  const entries = buildLauncherEntries({
    apps: [
      { capsuleId: "cap_yuru", label: "Yurucommu", iconUrl: "https://store.example/yuru.png", state: "installed" },
      { capsuleId: "cap_office", label: "Office", state: "installed" },
      { capsuleId: "cap_noscreen", label: "Storage", state: "installed" },
      { capsuleId: "cap_unloaded", label: "Git", state: "installed" },
    ],
    screens: new Map([
      screensFor("cap_yuru", [
        { interfaceId: "web", name: "web", title: "ゆるこみゅ", url: "https://yuru.example/" },
      ]),
      screensFor("cap_office", [
        { interfaceId: "docs", name: "docs", title: "Docs", sortOrder: 2, url: "https://office.example/docs", icon: { kind: "glyph", glyph: "📄" } },
        { interfaceId: "sheets", name: "sheets", sortOrder: 1, url: "https://office.example/sheets" },
      ]),
      screensFor("cap_noscreen", []),
    ]),
    custom: [
      { id: "custom-0001", url: "https://news.example/", title: "News", addedAt: "2026-10-29T00:00:00.000Z" },
      { id: "custom-0002", url: "http://insecure.example/", title: "Old", addedAt: "2026-10-29T00:00:00.000Z" },
    ],
  });
  expect(entries.map(({ key, label, url, host, icon, fallbackGlyph, source }) => ({
    key,
    label,
    url,
    host,
    icon,
    fallbackGlyph,
    source,
  }))).toEqual([
    {
      key: "app:cap_yuru:web",
      // One screen: the app's own name, and the store icon when the screen has none.
      label: "Yurucommu",
      url: "https://yuru.example/",
      host: "yuru.example",
      icon: { kind: "image", url: "https://store.example/yuru.png" },
      fallbackGlyph: "Y",
      source: "takosumi",
    },
    {
      key: "app:cap_office:sheets",
      label: "Office sheets",
      url: "https://office.example/sheets",
      host: "office.example",
      icon: undefined,
      fallbackGlyph: "O",
      source: "takosumi",
    },
    {
      key: "app:cap_office:docs",
      label: "Docs",
      url: "https://office.example/docs",
      host: "office.example",
      icon: { kind: "glyph", glyph: "📄" },
      fallbackGlyph: "D",
      source: "takosumi",
    },
    {
      key: "custom:custom-0001",
      label: "News",
      url: "https://news.example/",
      host: "news.example",
      icon: undefined,
      fallbackGlyph: "N",
      source: "custom",
    },
  ]);
});

test("a screens view for another Capsule is not shown under the listed app", () => {
  const entries = buildLauncherEntries({
    apps: [{ capsuleId: "cap_a", label: "A", state: "installed" }],
    screens: new Map([
      ["cap_a", { capsuleId: "cap_b", complete: true, screens: [{ interfaceId: "w", name: "w", url: "https://b.example/" }] }],
    ]),
    custom: [],
  });
  expect(entries).toEqual([]);
});

test("app screen deep links carry only an https URL", () => {
  const link = appScreenDeepLink("https://yuru.example/notifications?x=1");
  expect(link).toBe("takosumi://app?url=https%3A%2F%2Fyuru.example%2Fnotifications%3Fx%3D1");
  expect(parseAppScreenDeepLink(link)).toEqual({
    ok: true,
    url: "https://yuru.example/notifications?x=1",
  });
  expect(parseAppScreenDeepLink("takosumi://app?url=http%3A%2F%2Fyuru.example")).toEqual({
    ok: false,
    error: "https のURLだけ開けます。",
  });
  expect(parseAppScreenDeepLink("takosumi://app")).toEqual({
    ok: false,
    error: "開くURLがありません。",
  });
  // Links for the shared shell pass through untouched.
  expect(parseAppScreenDeepLink("takosumi://oauth/callback?code=x")).toBeUndefined();
  expect(parseAppScreenDeepLink("takosumi://open?path=/runs/1")).toBeUndefined();
  expect(parseAppScreenDeepLink("https://app.takosumi.com")).toBeUndefined();
  expect(parseAppScreenDeepLink("not a url")).toBeUndefined();
  expect(() => appScreenDeepLink("http://yuru.example")).toThrow();
});

test("a deep-linked URL reuses the known app's screen, never a name from the link", () => {
  const entries = buildLauncherEntries({
    apps: [{ capsuleId: "cap_yuru", label: "Yurucommu", state: "installed" }],
    screens: new Map([
      screensFor("cap_yuru", [{ interfaceId: "web", name: "web", url: "https://yuru.example/" }]),
    ]),
    custom: [],
  });
  expect(resolveAppScreenTarget("https://yuru.example/notifications", entries)).toEqual({
    key: "app:cap_yuru:web",
    label: "Yurucommu",
    url: "https://yuru.example/notifications",
    fallbackGlyph: "Y",
  });
  expect(resolveAppScreenTarget("https://other.example/x", entries)).toEqual({
    key: "origin:https://other.example",
    label: "other.example",
    url: "https://other.example/x",
    fallbackGlyph: "O",
  });
  expect(resolveAppScreenTarget("https://evil.yuru.example/", entries).key).toBe(
    "origin:https://evil.yuru.example",
  );
});

test("every app screen opens in the one shared browsing profile", () => {
  const entries = buildLauncherEntries({
    apps: [
      { capsuleId: "cap_a", label: "A", state: "installed" },
      { capsuleId: "cap_b", label: "B", state: "installed" },
    ],
    screens: new Map([
      screensFor("cap_a", [{ interfaceId: "w", name: "w", url: "https://a.example/" }]),
      screensFor("cap_b", [{ interfaceId: "w", name: "w", url: "https://b.other.example/" }]),
    ]),
    custom: [{ id: "custom-0003", url: "https://accounts.takosumi.example/", title: "Takosumi", addedAt: "2026-10-29T00:00:00.000Z" }],
  });
  const requests = entries.map((entry) => appScreenOpenRequest(entry));
  expect(new Set(requests.map((request) => request.profile))).toEqual(
    new Set([APP_SCREEN_PROFILE]),
  );
  expect(APP_SCREEN_PROFILE).toBe("shared");
  expect(requests.map((request) => request.key)).toEqual([
    "app:cap_a:w",
    "app:cap_b:w",
    "custom:custom-0003",
  ]);
  expect(appScreenOpenRequest(entries[0]!, "iVBORw0KGgo=")).toEqual({
    key: "app:cap_a:w",
    url: "https://a.example/",
    title: "A",
    iconPng: "iVBORw0KGgo=",
    profile: "shared",
  });
  expect(() =>
    appScreenOpenRequest({ key: "k", label: "x", url: "http://a.example/" }),
  ).toThrow("https");
});

test("a home refresh keeps unchanged tiles and replaces changed ones", () => {
  const build = (title: string) =>
    buildLauncherEntries({
      apps: [
        { capsuleId: "cap_a", label: "A", state: "installed" },
        { capsuleId: "cap_b", label: title, state: "installed" },
      ],
      screens: new Map([
        screensFor("cap_a", [{ interfaceId: "w", name: "w", url: "https://a.example/" }]),
        screensFor("cap_b", [{ interfaceId: "w", name: "w", url: "https://b.example/" }]),
      ]),
      custom: [],
    });
  const first = build("B");
  const same = stabilizeLauncherEntries(first, build("B"));
  expect(same[0]).toBe(first[0]!);
  expect(same[1]).toBe(first[1]!);
  const renamed = stabilizeLauncherEntries(first, build("B2"));
  expect(renamed[0]).toBe(first[0]!);
  expect(renamed[1]).not.toBe(first[1]!);
  expect(renamed[1]?.label).toBe("B2");
});
