import { expect, test } from "bun:test";
import { readdir } from "node:fs/promises";
import { INTERFACE_CLIENT_BLOCKED_SCHEMES } from "../../takosumi/contract/interface-clients.ts";

/**
 * An app screen shows other people's web pages. It must not reach the Tauri
 * shell: no Tauri capability (no IPC for remote URLs), no JavaScript bridge
 * into native plugins, and none of the shell's own hosts. These checks keep
 * the configuration and the native webview factories that way.
 */

const root = new URL("../src-tauri/", import.meta.url);

interface Capability {
  readonly identifier: string;
  readonly windows?: readonly string[];
  readonly webviews?: readonly string[];
  readonly remote?: unknown;
  readonly local?: boolean;
  readonly permissions: readonly unknown[];
}

async function capabilities(): Promise<Capability[]> {
  const directory = new URL("capabilities/", root);
  const files = (await readdir(directory)).filter((name) => /\.(json|toml)$/u.test(name));
  expect(files.every((name) => name.endsWith(".json"))).toBe(true);
  return await Promise.all(
    files.map(async (name) => (await Bun.file(new URL(name, directory)).json()) as Capability),
  );
}

test("capabilities apply only to the shell's main window, never to remote URLs", async () => {
  const all = await capabilities();
  expect(all.map((capability) => capability.identifier).sort()).toEqual(["default", "mobile"]);
  for (const capability of all) {
    expect({ id: capability.identifier, windows: capability.windows }).toEqual({
      id: capability.identifier,
      windows: ["main"],
    });
    expect(capability.webviews).toBeUndefined();
    // `remote` would grant IPC to pages loaded from those URLs.
    expect(capability.remote).toBeUndefined();
    expect(capability.local).not.toBe(false);
  }
  const config = (await Bun.file(new URL("tauri.conf.json", root)).json()) as {
    app: {
      windows: Array<{ label: string; url?: string }>;
      security: Record<string, unknown>;
      withGlobalTauri?: boolean;
    };
  };
  // The shell is the only Tauri webview, and it loads only bundled files.
  expect(config.app.windows.map((window) => window.label)).toEqual(["main"]);
  expect(config.app.windows[0]?.url).toBeUndefined();
  expect(config.app.security.capabilities).toBeUndefined();
  expect(config.app.security.dangerousDisableAssetCspModification).toBeUndefined();
  expect(config.app.withGlobalTauri).not.toBe(true);
});

test("the app-screen plugin's commands are granted to the shell's capability only", async () => {
  const all = await capabilities();
  const granting = all.filter((capability) =>
    capability.permissions.some(
      (permission) => typeof permission === "string" && permission.startsWith("app-screen:"),
    ),
  );
  expect(granting.map((capability) => capability.identifier)).toEqual(["mobile"]);
  const permissions = await Bun.file(
    new URL("plugins/app-screen/permissions/default.toml", root),
  ).text();
  const build = await Bun.file(new URL("plugins/app-screen/build.rs", root)).text();
  for (const command of [
    "capabilities",
    "open",
    "pin_shortcut",
    "open_client",
    "open_browser_view",
    "open_web",
    "take_pending_action",
    "register_listener",
  ]) {
    const permission = `"allow-${command.replaceAll("_", "-")}"`;
    expect({
      command,
      built: build.includes(`"${command}"`),
      permitted: permissions.includes(permission),
    }).toEqual({ command, built: true, permitted: true });
  }
});

async function sources(directory: URL, extension: string): Promise<Map<string, string>> {
  const found = new Map<string, string>();
  for (const name of await readdir(directory, { recursive: true })) {
    if (!name.endsWith(extension)) continue;
    found.set(name, await Bun.file(new URL(name, directory)).text());
  }
  return found;
}

test("Android app screens are plain WebViews with no JavaScript bridge", async () => {
  const kotlin = await sources(
    new URL("plugins/app-screen/android/src/main/java/", root),
    ".kt",
  );
  expect(kotlin.size).toBeGreaterThan(3);
  for (const [name, source] of kotlin) {
    for (const bridge of [
      "addJavascriptInterface",
      "addWebMessageListener",
      "addDocumentStartJavaScript",
      "createWebMessageChannel",
      "postWebMessage",
      "evaluateJavascript",
      "RustWebView",
      "app.tauri.WebView",
    ]) {
      expect({ name, bridge, found: source.includes(bridge) }).toEqual({ name, bridge, found: false });
    }
  }
  const registry = kotlin.get("AppScreenRegistry.kt")!;
  expect({
    plainWebView: registry.includes("val webView = WebView(context)"),
    noFiles: registry.includes("allowFileAccess = false"),
    noContent: registry.includes("allowContentAccess = false"),
    pushPlaceholder: registry.includes("Push API"),
  }).toEqual({ plainWebView: true, noFiles: true, noContent: true, pushPlaceholder: false });
});

test("iOS app screens are plain WKWebViews with no script message handlers", async () => {
  const swift = await sources(new URL("plugins/app-screen/ios/Sources/", root), ".swift");
  expect(swift.size).toBeGreaterThan(2);
  for (const [name, source] of swift) {
    for (const bridge of [
      "WKScriptMessageHandler",
      "addScriptMessageHandler",
      "userContentController.add(",
      "WKUserScript",
      "addUserScript",
      "evaluateJavaScript",
      "setURLSchemeHandler",
    ]) {
      expect({ name, bridge, found: source.includes(bridge) }).toEqual({ name, bridge, found: false });
    }
  }
  const controller = swift.get("AppScreenViewController.swift")!;
  // A fresh configuration: never the Tauri webview's, which carries the IPC.
  expect({
    freshConfiguration: controller.includes("let configuration = WKWebViewConfiguration()"),
    pushPlaceholder: controller.includes("Push API"),
  }).toEqual({ freshConfiguration: true, pushPlaceholder: false });
});

test("the Rust plugin creates no Tauri webview for app screens", async () => {
  const rust = await sources(new URL("plugins/app-screen/src/", root), ".rs");
  for (const [name, source] of rust) {
    for (const builder of ["WebviewBuilder", "WebviewWindowBuilder", "WebviewUrl", "add_child"]) {
      expect({ name, builder, found: source.includes(builder) }).toEqual({
        name,
        builder,
        found: false,
      });
    }
  }
});

test("the native policies keep the shared credential list and the shell hosts", async () => {
  const rules = (await Bun.file(
    new URL("plugins/app-screen/resume-url-cases.json", root),
  ).json()) as { credentialQueryNames: string[] };
  const kotlin = await Bun.file(
    new URL("plugins/app-screen/android/src/main/java/AppScreenRules.kt", root),
  ).text();
  const swift = await Bun.file(
    new URL("plugins/app-screen/ios/Sources/AppScreenPolicy.swift", root),
  ).text();
  for (const name of rules.credentialQueryNames) {
    expect({ name, kotlin: kotlin.includes(`"${name}"`), swift: swift.includes(`"${name}"`) })
      .toEqual({ name, kotlin: true, swift: true });
  }
  for (const host of ["tauri.localhost", "ipc.localhost", "asset.localhost"]) {
    expect({ host, kotlin: kotlin.includes(`"${host}"`), swift: swift.includes(`"${host}"`) })
      .toEqual({ host, kotlin: true, swift: true });
  }
});

test("the native re-checks refuse every scheme Takosumi refuses for native apps", async () => {
  const native = {
    rust: await Bun.file(new URL("plugins/app-screen/src/models.rs", root)).text(),
    kotlin: await Bun.file(
      new URL("plugins/app-screen/android/src/main/java/AppScreenRules.kt", root),
    ).text(),
    swift: await Bun.file(new URL("plugins/app-screen/ios/Sources/AppScreenPolicy.swift", root)).text(),
  };
  expect(INTERFACE_CLIENT_BLOCKED_SCHEMES.size).toBeGreaterThan(40);
  for (const scheme of INTERFACE_CLIENT_BLOCKED_SCHEMES) {
    expect({
      scheme,
      rust: native.rust.includes(`"${scheme}"`),
      kotlin: native.kotlin.includes(`"${scheme}"`),
      swift: native.swift.includes(`"${scheme}"`),
    }).toEqual({ scheme, rust: true, kotlin: true, swift: true });
  }
});
