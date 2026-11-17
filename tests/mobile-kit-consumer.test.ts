import { expect, test } from "bun:test";
import { readFileSync, readdirSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import ts from "typescript";
import {
  createMobileClientController,
  createTauriMobileDefaultProductBridge,
  loadMobileAccounts,
  mobileAccountSessionStorageKey,
  saveMobileAccountSession,
  type CreateMobileClientControllerOptions,
  type MobileRetiredAccountScope,
  type NativeBridge,
} from "@takosjp/mobile-kit";
import { productAdapter } from "../src/product.ts";
import { createPushManager, RELAY_DEVICE_STORAGE_KEY } from "../src/push-manager.ts";
import { withAccountRemovalHook } from "../src/push-account-hook.ts";
import { createFakeInstance, createFakeNative, createPushWorld, memorySecureStore, memoryStore, RELAY_ORIGIN } from "./support/push-world.ts";

const appRoot = resolve(import.meta.dir, "..");
const main = readFileSync(resolve(appRoot, "src/main.tsx"), "utf8");
function section(source: string, from: string, to?: string): string {
  const start = source.indexOf(from), end = to ? source.indexOf(to, start) : source.length;
  if (start < 0 || end < 0) throw new Error("actual product function boundary changed");
  return source.slice(start, end);
}
function compile(source: string): string {
  return ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.React, jsxFactory: "h" } }).outputText;
}
type ShellOptions = Pick<CreateMobileClientControllerOptions<object>, "adapter" | "sessionUnlock" | "onAccountScopeRetired"> & { createNativeBridge: () => NativeBridge };
/** Execute the product's exact options factory; the deferred JSX is not rendered. */
function shellOptions(bridge: NativeBridge, manager: ReturnType<typeof createPushManager>): ShellOptions {
  let captured: ShellOptions | undefined;
  const factory = new Function("renderMobileClientApp", "productAdapter", "loadHome", "mobileHomeTitle", "metrics", "actions",
    `${compile(section(main, "function renderShell(", "\nfunction renderUnsupportedDesktop"))}; return renderShell;`);
  factory((options: ShellOptions) => { captured = options; }, productAdapter, async () => ({}), () => undefined, [], [])({
    nativeBridge: bridge, push: { manager }, launcher: {}, opener: {}, routing: {},
  });
  if (!captured) throw new Error("actual shell did not provide options");
  return captured;
}

test("the configured Kit source and installed runtime projection agree", () => {
  const refs = ["ci.yml", "windows-real-sites.yml"].map(name => {
    const source = readFileSync(resolve(appRoot, ".github/workflows", name), "utf8");
    const refs = [...source.matchAll(/^\s*MOBILE_KIT_SOURCE_REF:\s*([a-f0-9]{40})\s*$/gm)];
    expect(refs).toHaveLength(1);
    return refs[0]![1];
  });
  expect(refs[0]).toBe(refs[1]);
  if (process.env.MOBILE_KIT_SOURCE_REF) expect(refs[0]).toBe(process.env.MOBILE_KIT_SOURCE_REF);
  const installed = dirname(Bun.resolveSync("@takosjp/mobile-kit", appRoot));
  const source = resolve(appRoot, "../mobile-kit/src");
  const files = readdirSync(source).filter(name => /\.(ts|tsx|css)$/.test(name));
  expect(files.length).toBeGreaterThan(20);
  for (const name of files) expect(readFileSync(resolve(installed, name))).toEqual(readFileSync(resolve(source, name)));
});

test("actual Kit retirement reaches only the product's matching saved subscription", async () => {
  for (const mode of ["missing", "mismatched", "valid", "row-write-refused", "device-secret-missing"] as const) {
    const a = createFakeInstance({ origin: "https://retired.example" });
    const b = createFakeInstance({ origin: "https://kept.example" });
    const world = createPushWorld([a, b]), native = createFakeNative(), storage = memoryStore(), secure = memorySecureStore();
    const requests: { method: string; url: string }[] = [];
    const fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = new Request(input, init);
      requests.push({ method: request.method, url: request.url });
      const body = request.method === "GET" || request.method === "HEAD" ? undefined : await request.text();
      return await world.fetch(request.url, { method: request.method, headers: request.headers,
        ...(body ? { body } : {}) });
    };
    const manager = createPushManager({ native: native.native, relayOrigin: RELAY_ORIGIN, store: storage, secureStore: secure,
      fetch, freshSession: async session => session, lang: () => "ja-JP" });
    const sa = a.session("alice"), sb = b.session("bob");
    const ra = await manager.enable(sa), rb = await manager.enable(sb);
    const base: NativeBridge = { capabilities: {} as never, storage, secureStore: secure, getLaunchPayload: async () => undefined, openExternalUrl: async () => {} };
    const rowA = (await saveMobileAccountSession({ adapter: productAdapter, nativeBridge: base, session: sa }))!;
    await saveMobileAccountSession({ adapter: productAdapter, nativeBridge: base, session: sb });
    const keyA = mobileAccountSessionStorageKey(productAdapter, rowA.id);
    if (mode === "missing" || mode === "row-write-refused" || mode === "device-secret-missing") secure.values.delete(keyA);
    if (mode === "mismatched") secure.values.set(keyA, JSON.stringify(sb));
    if (mode === "device-secret-missing") secure.values.delete(RELAY_DEVICE_STORAGE_KEY);
    const failure = new Error("fixture account row write refused");
    const bridge = withAccountRemovalHook(mode === "row-write-refused" ? { ...base, storage: { ...storage,
      async set(key, value) { if (key.endsWith(".accounts")) throw failure; await storage.set(key, value); },
    } } : base, productAdapter, session => manager.accountRemoving(session));
    const options = shellOptions(bridge, manager);
    const retired: MobileRetiredAccountScope[] = [];
    const controller = createMobileClientController({ adapter: options.adapter, nativeBridge: options.createNativeBridge(),
      loadHome: async () => ({}), sessionUnlock: options.sessionUnlock, fetch,
      onAccountScopeRetired: async scope => { retired.push(scope); await options.onAccountScopeRetired?.(scope); },
    });
    try {
      await controller.start();
      if (mode === "row-write-refused") {
        await expect(controller.signOutAccount(rowA.id)).rejects.toBe(failure);
        expect(retired).toHaveLength(0);
        expect(await manager.record(sa)).toBeDefined();
      } else {
        await controller.signOutAccount(rowA.id);
        await Promise.resolve();
        expect(await manager.record(sa)).toBeUndefined();
        expect(retired.map(scope => scope.id)).toEqual([rowA.id]);
        expect(native.keys.has(ra.subscriptionId)).toBe(false);
        expect((await loadMobileAccounts({ adapter: productAdapter, nativeBridge: bridge })).accounts.some(row => row.id === rowA.id)).toBe(false);
        // No token from a missing/mismatched slot is fabricated for instance cleanup.
        expect(a.requests.filter(request => request.method === "DELETE")).toHaveLength(mode === "valid" ? 1 : 0);
        const requestsBefore = requests.length;
        await options.onAccountScopeRetired?.(retired[0]!);
        expect(await manager.record(sa)).toBeUndefined();
        expect(requests).toHaveLength(requestsBefore);
        expect(requests.filter(request => request.method === "DELETE" && request.url.startsWith(RELAY_ORIGIN))).toHaveLength(mode === "device-secret-missing" ? 0 : 1);
      }
      expect((await manager.record(sb))?.subscriptionId).toBe(rb.subscriptionId);
      expect(native.keys.has(rb.subscriptionId)).toBe(true);
      expect(b.requests.filter(request => request.method === "DELETE")).toHaveLength(0);
      expect(secure.values.has(RELAY_DEVICE_STORAGE_KEY)).toBe(mode !== "device-secret-missing");
    } finally { controller.stop(); }
  }
});

test("the product bridge and same controller recover rejected initialization only on explicit Retry", async () => {
  const installed = dirname(Bun.resolveSync("@takosjp/mobile-kit", appRoot));
  // Execute the shell's exact helper body with its peer resolved from this App,
  // without requiring a second dependency install in the source-only Kit checkout.
  const retrySource = readFileSync(resolve(installed, "startup-retry.ts"), "utf8");
  const retryModule = ts.transpileModule(retrySource, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS } }).outputText;
  const { createStartupRetry } = new Function("require", "exports", `${retryModule}; return exports;`)(createRequire(resolve(appRoot, "package.json")), {}) as {
    createStartupRetry(run: () => Promise<unknown>): { failed(): boolean; attempt(): Promise<void>; dispose(): void };
  };
  const nativeSource = readFileSync(resolve(appRoot, "src/native.ts"), "utf8");
  for (const failedPort of ["store", "stronghold"] as const) {
    let allow = false, loads = 0, vaultLoads = 0, live = 0, maxLive = 0;
    const failure = new Error(`fixture ${failedPort} initialization refused`), traces: unknown[][] = [];
    const oldFetch = globalThis.fetch, oldError = console.error;
    const noCall = async () => {};
    const vaultStore = { get: async () => null, insert: noCall, remove: noCall };
    const ports = {
      isUnsupportedTauriDesktop: () => false, detectTauriRuntime: () => true,
      tauriFetch: async () => { throw new Error("unexpected fixture network"); }, openUrl: noCall,
      createTauriMobileDefaultProductBridge: (options: Parameters<typeof createTauriMobileDefaultProductBridge>[0]) =>
        createTauriMobileDefaultProductBridge({ ...options, isTauriRuntime: () => true }),
      productAdapter, invoke: async () => "fixture-stronghold-password-for-local-control-only",
      appDataDir: async () => "/fixture", join: async (...parts: string[]) => parts.join("/"), getCurrent: async () => [],
      onOpenUrl: async () => { live++; maxLive = Math.max(maxLive, live); return () => { live--; }; },
      load: async () => { loads++; if (!allow && failedPort === "store") throw failure;
        return { get: async () => undefined, set: noCall, delete: noCall, save: noCall }; },
      Stronghold: { load: async () => { vaultLoads++; if (!allow && failedPort === "stronghold") throw failure;
        return { loadClient: async () => ({ getStore: () => vaultStore }), createClient: async () => ({ getStore: () => vaultStore }), save: noCall }; } },
      platform: () => "android", isPermissionGranted: async () => false, requestPermission: async () => "denied", sendNotification: () => {},
      scan: noCall, Format: { QRCode: "QR_CODE" }, checkPermissions: noCall, requestPermissions: noCall, authenticate: noCall, writeText: noCall,
    };
    const keys = Object.keys(ports);
    const bridgeFactory = new Function("ports", `const { ${keys.join(", ")} } = ports; ${compile(section(nativeSource, "export function createProductNativeBridge()" )).replace("export function", "function")}; return createProductNativeBridge;`)(ports) as () => NativeBridge;
    console.error = (...args: unknown[]) => { traces.push(args); };
    const bridge = bridgeFactory();
    const options = shellOptions(bridge, {} as ReturnType<typeof createPushManager>);
    const controller = createMobileClientController({ adapter: options.adapter, nativeBridge: options.createNativeBridge(),
      loadHome: async () => ({}), sessionUnlock: options.sessionUnlock });
    const retry = createStartupRetry(() => controller.start());
    try {
      await retry.attempt();
      expect(retry.failed()).toBe(true);
      expect(traces[0]?.[1]).toBe(failure);
      const attempts = [loads, vaultLoads];
      await Promise.resolve();
      expect([loads, vaultLoads]).toEqual(attempts);
      allow = true;
      await retry.attempt();
      expect(retry.failed()).toBe(false);
      expect(options.createNativeBridge()).toBe(bridge);
      expect(failedPort === "store" ? loads : vaultLoads).toBeGreaterThan(attempts[failedPort === "store" ? 0 : 1]!);
      expect(maxLive).toBe(1);
      expect(live).toBe(1);
    } finally { retry.dispose(); controller.stop(); globalThis.fetch = oldFetch; console.error = oldError; }
  }
});
