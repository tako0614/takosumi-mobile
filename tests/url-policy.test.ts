import { expect, test } from "bun:test";
import policyCases from "../src-tauri/plugins/app-screen/navigation-policy-cases.json";
import resumeCases from "../src-tauri/plugins/app-screen/resume-url-cases.json";
import {
  accountsOrigin,
  appScreenAllowedOrigins,
  CREDENTIAL_QUERY_NAMES,
  decideAppScreenNavigation,
  displayHost,
  intentTargetScheme,
  isLaunchableAppScreenUrl,
  isRememberableAppScreenUrl,
  normalizeUserAppUrl,
  RESUME_URL_MAX_LENGTH,
  sameOrigin,
  screenOrigin,
} from "../src/url-policy.ts";

test("every shared navigation case matches the TypeScript policy", () => {
  expect(policyCases.cases.length).toBeGreaterThan(40);
  for (const item of policyCases.cases) {
    const allowedOrigins = (item as { allowedOrigins?: string[] }).allowedOrigins;
    const decision = decideAppScreenNavigation(item.url, {
      mainFrame: item.mainFrame,
      ...(allowedOrigins ? { allowedOrigins } : {}),
    });
    expect({ url: item.url, allowedOrigins, kind: decision.kind }).toEqual({
      url: item.url,
      allowedOrigins,
      kind: item.decision as typeof decision.kind,
    });
  }
});

test("every shared resume case matches the TypeScript rule", () => {
  expect(resumeCases.cases.length).toBeGreaterThan(15);
  for (const item of resumeCases.cases) {
    expect({ url: item.url, remember: isRememberableAppScreenUrl(item.url, item.appOrigin) })
      .toEqual({ url: item.url, remember: item.remember });
  }
  const long = `https://app.example/${"a".repeat(RESUME_URL_MAX_LENGTH)}`;
  expect(isRememberableAppScreenUrl(long, "https://app.example")).toBe(false);
});

test("pages of other origins go to the browser view; the app and Accounts stay", () => {
  const allowedOrigins = appScreenAllowedOrigins({
    appUrl: "https://yurucommu.example/home?tab=1",
    accountsOrigin: "https://accounts.takosumi.example",
  });
  expect(allowedOrigins).toEqual([
    "https://yurucommu.example",
    "https://accounts.takosumi.example",
  ]);
  const decide = (url: string, mainFrame = true) =>
    decideAppScreenNavigation(url, { mainFrame, allowedOrigins });
  expect(decide("https://yurucommu.example/notifications")).toEqual({ kind: "in-app" });
  expect(decide("https://accounts.takosumi.example/oauth/authorize?x=1")).toEqual({
    kind: "in-app",
  });
  // Google forbids OAuth in embedded webviews: the browser view signs in.
  expect(decide("https://accounts.google.com/o/oauth2/v2/auth")).toEqual({
    kind: "browser-view",
    reason: "other-origin",
  });
  expect(decide("https://yurucommu.example.evil.example/")).toEqual({
    kind: "browser-view",
    reason: "other-origin",
  });
  // Subframes (embedded widgets) keep their own rules.
  expect(decide("https://accounts.google.com/gsi/iframe", false)).toEqual({ kind: "in-app" });
  // The scheme rules come first.
  expect(decide("http://yurucommu.example/").kind).toBe("external-confirm");
  expect(decide("takosumi://oauth/callback?code=x").kind).toBe("block");
});

test("the Tauri shell's own hosts never load in an app screen", () => {
  for (const url of [
    "https://tauri.localhost/",
    "https://ipc.localhost/plugin%3Aapp-screen%7Copen",
    "https://asset.localhost/x",
    "https://TAURI.localhost./",
  ]) {
    expect(decideAppScreenNavigation(url, { mainFrame: true })).toEqual({
      kind: "block",
      reason: "shell-host",
    });
    expect(isLaunchableAppScreenUrl(url)).toBe(false);
  }
  expect(normalizeUserAppUrl("tauri.localhost").ok).toBe(false);
});

test("the Accounts origin is the https issuer, else the instance itself", () => {
  expect(
    accountsOrigin({
      hostUrl: "https://app.takosumi.example",
      oidcIssuer: "https://accounts.takosumi.example/",
    }),
  ).toBe("https://accounts.takosumi.example");
  expect(accountsOrigin({ hostUrl: "https://self.example:8443/" })).toBe(
    "https://self.example:8443",
  );
  expect(accountsOrigin({ hostUrl: "http://localhost:8787" })).toBeUndefined();
  expect(screenOrigin("https://APP.example:443/x")).toBe("https://app.example");
  expect(screenOrigin("about:blank")).toBeUndefined();
  expect(
    appScreenAllowedOrigins({ appUrl: "https://a.example/", accountsOrigin: "https://a.example" }),
  ).toEqual(["https://a.example"]);
});

test("resume never keeps a credential Takosumi itself would refuse", async () => {
  // Every name Takosumi's hasCredentialQueryParams refuses is refused here too.
  const contract = await Bun.file(
    new URL("../../takosumi/contract/interface-display.ts", import.meta.url),
  ).text();
  const block = /const CREDENTIAL_QUERY_NAMES = new Set\(\[([\s\S]*?)\]\)/u.exec(contract)?.[1];
  expect(block).toBeDefined();
  const serverNames = [...block!.matchAll(/"([a-z0-9]+)"/gu)].map((match) => match[1]!);
  expect(serverNames.length).toBeGreaterThan(20);
  for (const name of serverNames) expect(CREDENTIAL_QUERY_NAMES.has(name)).toBe(true);
  for (const name of ["code", "idtoken", "samlresponse"]) {
    expect(CREDENTIAL_QUERY_NAMES.has(name)).toBe(true);
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
  // As Intent.parseUri: parameters after the last '#', else the data's scheme.
  expect(intentTargetScheme("intent://x/#frag#Intent;scheme=takosumi;end")).toBe("takosumi");
  expect(intentTargetScheme("intent:takosumi://oauth/callback#Intent;end")).toBe("takosumi");
  expect(intentTargetScheme("intent://scan/#Intent;end")).toBeUndefined();
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
  expect(isLaunchableAppScreenUrl("https://app.example/#/inbox")).toBe(true);
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
