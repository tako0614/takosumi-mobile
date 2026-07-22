import { expect, test } from "bun:test";
import { productAdapter } from "../src/product.ts";

test("Takosumi mobile owns the Takosumi PKCE client identity", () => {
  expect(productAdapter.product).toBe("takosumi");
  expect(productAdapter.mobileScheme).toBe("takosumi");
  expect(productAdapter.oidcScopes).toEqual([
    "openid",
    "profile",
    "offline_access",
    "capsules:read",
    "capsules:write",
  ]);
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
  ).json()) as { identifier: string };
  expect(config.identifier).toBe("com.takosumi");
});
