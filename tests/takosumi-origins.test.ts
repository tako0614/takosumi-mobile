import { expect, test } from "bun:test";
import { mobileAccountListStorageKey } from "@takosjp/mobile-kit";
import type { AppScreenOpenRequest } from "../src/app-screens.ts";
import { createLauncherController } from "../src/launcher-controller.ts";
import { productAdapter } from "../src/product.ts";
import {
  createDeepLinkOpener,
  KNOWN_TAKOSUMI_ORIGINS_STORAGE_KEY,
  MAX_KNOWN_TAKOSUMI_ORIGINS,
  mergeTakosumiOrigins,
  readKnownTakosumiOrigins,
  rememberTakosumiOrigins,
  savedAccountOrigins,
} from "../src/takosumi-origins.ts";

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

const accountList = (hosts: string[]) =>
  JSON.stringify({
    version: 1,
    accounts: hosts.map((hostUrl, index) => ({
      id: `acct_${index}`,
      hostUrl,
      product: "takosumi",
      lastUsedAt: "2026-10-30T00:00:00.000Z",
    })),
  });

test("known Takosumi origins combine every session seen and every saved account", async () => {
  const store = memoryStore({
    [mobileAccountListStorageKey(productAdapter)]: accountList([
      "https://app.takosumi.example/",
      "https://self.example:8443",
      "http://localhost:8787",
    ]),
  });
  expect(await savedAccountOrigins(store, productAdapter)).toEqual([
    "https://app.takosumi.example",
    "https://self.example:8443",
  ]);
  await rememberTakosumiOrigins(store, ["https://accounts.takosumi.example", "https://app.takosumi.example"]);
  expect(await readKnownTakosumiOrigins({ store, adapter: productAdapter })).toEqual([
    "https://accounts.takosumi.example",
    "https://app.takosumi.example",
    "https://self.example:8443",
  ]);
  // Bounded, deduplicated, https origins only.
  const many = Array.from({ length: 40 }, (_, index) => `https://i${index}.example`);
  expect(mergeTakosumiOrigins(many, ["https://i0.example", "http://x.example", "https://a.example/path"]).length)
    .toBe(MAX_KNOWN_TAKOSUMI_ORIGINS);
  store.values.set(KNOWN_TAKOSUMI_ORIGINS_STORAGE_KEY, "{not json");
  expect(await readKnownTakosumiOrigins({ store, adapter: productAdapter })).toEqual([
    "https://app.takosumi.example",
    "https://self.example:8443",
  ]);
});

function harness(input: {
  readonly current?: readonly string[];
  readonly known?: readonly string[];
  readonly saved?: boolean;
}) {
  const opened: AppScreenOpenRequest[] = [];
  const browserViews: string[] = [];
  let current = input.current;
  const launcher = createLauncherController({
    opener: {
      capabilities: async () => ({ inApp: true, pinShortcut: false }),
      open: async (request) => void opened.push(request),
      pinShortcut: async () => false,
      openClient: async () => "not_installed" as const,
      openBrowserView: async (url) => {
        browserViews.push(url);
        return "browser-view" as const;
      },
      openWeb: async () => undefined,
      takePendingAction: async () => undefined,
      onActionPending: async () => () => undefined,
    },
    fetchIcon: async () => undefined,
    rasterize: async () => undefined,
    iconWaitMs: 1,
  });
  const links = createDeepLinkOpener({
    openUrl: (url, options) => launcher.openUrl(url, options),
    platform: "android",
    currentOrigins: () => current,
    knownOrigins: async () => input.known ?? [],
    hasSavedAccounts: async () => input.saved ?? false,
  });
  return {
    links,
    opened,
    browserViews,
    restore: (origins: readonly string[]) => {
      current = origins;
    },
  };
}

test("a dashboard deep link without a session never opens as an app", async () => {
  // Signed out (or before restore) with the instance known from earlier use.
  const { links, opened, browserViews } = harness({ known: ["https://app.takosumi.example"] });
  await links.open("https://app.takosumi.example/apps");
  await links.open("https://yurucommu.example/notes/1");
  expect(browserViews).toEqual(["https://app.takosumi.example/apps"]);
  expect(opened.map((request) => [request.url, request.accountsOrigins, request.scope])).toEqual([
    ["https://yurucommu.example/notes/1", ["https://app.takosumi.example"], undefined],
  ]);
});

test("with saved accounts a deep link waits for the session, then uses every Takosumi origin", async () => {
  const { links, opened, browserViews, restore } = harness({
    known: ["https://other.takosumi.example"],
    saved: true,
  });
  await links.open("https://accounts.takosumi.example/account");
  expect(opened).toEqual([]);
  expect(browserViews).toEqual([]);
  // The restored session names its issuer, unknown until now.
  restore(["https://accounts.takosumi.example", "https://app.takosumi.example"]);
  await links.sessionReady();
  expect(browserViews).toEqual(["https://accounts.takosumi.example/account"]);
  await links.sessionReady();
  expect(browserViews.length).toBe(1);
  // Another saved instance's dashboard is a Takosumi page too.
  await links.open("https://other.takosumi.example/");
  await links.open("https://yurucommu.example/");
  expect(browserViews).toEqual(["https://accounts.takosumi.example/account", "https://other.takosumi.example/"]);
  expect(opened[0]?.accountsOrigins).toEqual([
    "https://accounts.takosumi.example",
    "https://app.takosumi.example",
    "https://other.takosumi.example",
  ]);
});
