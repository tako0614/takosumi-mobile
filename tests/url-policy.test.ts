import { expect, test } from "bun:test";
import policyCases from "../src-tauri/plugins/app-screen/navigation-policy-cases.json";
import {
  decideAppScreenNavigation,
  displayHost,
  intentTargetScheme,
  isLaunchableAppScreenUrl,
  normalizeUserAppUrl,
  sameOrigin,
} from "../src/url-policy.ts";

test("every shared navigation case matches the TypeScript policy", () => {
  expect(policyCases.cases.length).toBeGreaterThan(20);
  for (const item of policyCases.cases) {
    const decision = decideAppScreenNavigation(item.url, {
      mainFrame: item.mainFrame,
    });
    expect({ url: item.url, mainFrame: item.mainFrame, kind: decision.kind }).toEqual({
      url: item.url,
      mainFrame: item.mainFrame,
      kind: item.decision as typeof decision.kind,
    });
  }
});

test("the main frame shows https only and hands other schemes to the OS after confirmation", () => {
  expect(decideAppScreenNavigation("https://a.example/", { mainFrame: true })).toEqual({
    kind: "in-app",
  });
  expect(decideAppScreenNavigation("http://a.example/", { mainFrame: true })).toEqual({
    kind: "external-confirm",
    reason: "insecure-http",
    scheme: "http",
  });
  expect(decideAppScreenNavigation("tel:+81-3-0000-0000", { mainFrame: true })).toEqual({
    kind: "external-confirm",
    reason: "other-scheme",
    scheme: "tel",
  });
  expect(decideAppScreenNavigation("mailto:a@b.example", { mainFrame: true })).toEqual({
    kind: "external-confirm",
    reason: "other-scheme",
    scheme: "mailto",
  });
});

test("pages cannot drive the shell or open local content", () => {
  expect(
    decideAppScreenNavigation("takosumi://oauth/callback?code=x&state=y", {
      mainFrame: true,
    }),
  ).toEqual({ kind: "block", reason: "shell-scheme" });
  expect(decideAppScreenNavigation("javascript:alert(1)", { mainFrame: true })).toEqual({
    kind: "block",
    reason: "local-content",
  });
  expect(decideAppScreenNavigation("file:///data/x", { mainFrame: true })).toEqual({
    kind: "block",
    reason: "local-content",
  });
  expect(
    decideAppScreenNavigation("https://bank.example@evil.example/", { mainFrame: true }),
  ).toEqual({ kind: "block", reason: "user-info" });
  // An Android intent: URI is judged by the scheme it would really open.
  expect(
    decideAppScreenNavigation(
      "intent://oauth/callback?code=x#Intent;scheme=takosumi;end",
      { mainFrame: true },
    ),
  ).toEqual({ kind: "block", reason: "shell-scheme" });
  expect(
    decideAppScreenNavigation("intent://x#Intent;scheme=file;end", { mainFrame: true }),
  ).toEqual({ kind: "block", reason: "local-content" });
  expect(intentTargetScheme("intent://scan/#Intent;package=a;scheme=ZXing;end")).toBe("zxing");
  expect(intentTargetScheme("intent://scan/")).toBeUndefined();
});

test("subframes never reach the OS", () => {
  expect(decideAppScreenNavigation("tel:123", { mainFrame: false })).toEqual({
    kind: "block",
    reason: "subframe-scheme",
  });
  expect(decideAppScreenNavigation("https://embed.example", { mainFrame: false })).toEqual({
    kind: "in-app",
  });
});

test("only https URLs are launchable", () => {
  expect(isLaunchableAppScreenUrl("https://app.example/path?x=1")).toBe(true);
  expect(isLaunchableAppScreenUrl("http://app.example/")).toBe(false);
  expect(isLaunchableAppScreenUrl("about:blank")).toBe(false);
  expect(isLaunchableAppScreenUrl("https://u:p@app.example/")).toBe(false);
  expect(isLaunchableAppScreenUrl("takosumi://app?url=https://x.example")).toBe(false);
});

test("typed URLs normalize to https and refuse other schemes", () => {
  expect(normalizeUserAppUrl("example.com")).toEqual({
    ok: true,
    url: "https://example.com/",
    host: "example.com",
  });
  expect(normalizeUserAppUrl("  https://Example.com/a?b=1#frag ")).toEqual({
    ok: true,
    url: "https://example.com/a?b=1",
    host: "example.com",
  });
  expect(normalizeUserAppUrl("example.com:8443/app")).toEqual({
    ok: true,
    url: "https://example.com:8443/app",
    host: "example.com:8443",
  });
  expect(normalizeUserAppUrl("http://example.com").ok).toBe(false);
  expect(normalizeUserAppUrl("ftp://example.com").ok).toBe(false);
  expect(normalizeUserAppUrl("javascript:alert(1)").ok).toBe(false);
  expect(normalizeUserAppUrl("https://user:pw@example.com").ok).toBe(false);
  expect(normalizeUserAppUrl("").ok).toBe(false);
  expect(normalizeUserAppUrl("exa mple.com").ok).toBe(false);
  expect(normalizeUserAppUrl("intranet").ok).toBe(false);
});

test("the top bar host is the ASCII host and origins compare exactly", () => {
  expect(displayHost("https://bücher.example/path")).toBe("xn--bcher-kva.example");
  expect(displayHost("https://app.example:8443/x")).toBe("app.example:8443");
  expect(displayHost("not a url")).toBe("");
  expect(sameOrigin("https://a.example/x", "https://a.example/y?z")).toBe(true);
  expect(sameOrigin("https://a.example/", "https://b.a.example/")).toBe(false);
  expect(sameOrigin("https://a.example/", "http://a.example/")).toBe(false);
  expect(sameOrigin("about:blank", "about:blank")).toBe(false);
});
