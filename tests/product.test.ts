import { expect, test } from "bun:test";
import { productAdapter } from "../src/product.ts";

test("Takosumi mobile owns the Takosumi PKCE client identity", () => {
  expect(productAdapter.product).toBe("takosumi");
  expect(productAdapter.mobileScheme).toBe("takosumi");
  expect(productAdapter.oidcScopes).toEqual([
    "openid",
    "offline_access",
    "workspaces:read",
    "capsules:read",
    "runs:read",
    "runs:approve",
    "runs:cancel",
  ]);
  expect(productAdapter.discoveryProduct).toBe("takosumi");
  expect(productAdapter.strictDiscoveryProduct).toBe(true);
});

test("Takosumi mobile consumes only the product-neutral sibling kit", async () => {
  const pkg = (await Bun.file(
    new URL("../package.json", import.meta.url),
  ).json()) as {
    dependencies?: Record<string, string>;
  };
  expect(pkg.dependencies?.["@takosjp/mobile-kit"]).toBe("file:../mobile-kit");
});

test("Takosumi mobile uses the stable com.takosumi application id", async () => {
  const config = (await Bun.file(
    new URL("../src-tauri/tauri.conf.json", import.meta.url),
  ).json()) as {
    identifier: string;
    app: { security: { csp: string | null } };
  };
  expect(config.identifier).toBe("com.takosumi");
  expect(config.app.security.csp).toContain("default-src 'self'");
  expect(config.app.security.csp).toContain("object-src 'none'");
  expect(config.app.security.csp).toContain("frame-ancestors 'none'");
  expect(config.app.security.csp).not.toBeNull();
});

test("native secrets remain device-bound and the desktop runtime is rejected", async () => {
  const android = await Bun.file(
    new URL(
      "../src-tauri/plugins/keystore/android/src/main/java/KeystorePlugin.kt",
      import.meta.url,
    ),
  ).text();
  const ios = await Bun.file(
    new URL(
      "../src-tauri/plugins/keystore/ios/Sources/KeystorePlugin.swift",
      import.meta.url,
    ),
  ).text();
  const native = await Bun.file(
    new URL("../src/native.ts", import.meta.url),
  ).text();

  expect(android).toContain(".setUnlockedDeviceRequired(true)");
  expect(ios).toContain("kSecAttrAccessibleWhenUnlockedThisDeviceOnly");
  expect(native).toContain("isUnsupportedTauriDesktop");
});
