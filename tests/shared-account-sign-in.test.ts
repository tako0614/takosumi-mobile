import { afterEach, expect, test } from "bun:test";
import {
  createMobileClientController,
  discoverHost,
  type FetchLike,
  type HostDiscovery,
  type MobileSession,
  type NativeBridge,
} from "@takosjp/mobile-kit";
import { loadHome } from "../src/api.ts";
import { productAdapter } from "../src/product.ts";
import { registerPusher } from "../src/push-instance.ts";
import {
  createSharedAccountSignIn,
  readSharedAccountSignIn,
  SHARED_SIGN_IN_PENDING_KEY,
  takosumiSessionFetch,
  TAKOSUMI_NATIVE_SESSION_HEADER,
} from "../src/shared-account-sign-in.ts";

// A fake shared-account host (app.example) whose issuer is auth.example, and a
// self-hosted instance (operator.example). The fake host behaves like the
// takosumi gateway: it accepts no bearer, relays {code, code_verifier, nonce}
// to "its RP", and takes the session only in the native session header.

const host = "https://app.example";
const issuer = "https://auth.example";
const clientId = "takosumi-mobile-v1";
const selfHost = "https://operator.example";
const sessionToken = `tns_${"S".repeat(43)}`;

// loadHome uses the app's global fetch (Tauri's HTTP plugin on devices).
const originalFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = originalFetch;
});

interface Recorded {
  readonly url: string;
  readonly method: string;
  readonly headers: Headers;
  readonly body?: string;
}

function sharedSignInDocument(overrides: Record<string, unknown> = {}) {
  return {
    issuer,
    client_id: clientId,
    redirect_uri: `${host}/auth/oidc/native/callback`,
    session_exchange: `${host}/api/v1/account/session/mobile-exchange`,
    session_header: "x-takosumi-native-session",
    scope: "openid email offline_access",
    ...overrides,
  };
}

async function sha256Base64Url(value: string): Promise<string> {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value)));
  let binary = "";
  for (const byte of digest) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/gu, "-").replace(/\//gu, "_").replace(/=+$/u, "");
}

function fakeNetwork(options: { exchangeStatus?: number; exchangeBody?: unknown } = {}) {
  const requests: Recorded[] = [];
  const authorize: { challenge?: string; nonce?: string } = {};
  let revoked = false;
  const fetch: FetchLike = async (input, init) => {
    const url = new URL(String(input));
    const headers = new Headers(init?.headers);
    const body = init?.body ? String(init.body) : undefined;
    requests.push({ url: url.toString(), method: init?.method ?? "GET", headers, body });
    if (url.origin === host && url.pathname === "/.well-known/takosumi") {
      return Response.json({
        product: "takosumi",
        name: "Takosumi",
        auth: { oidc: true, password: false },
        apiBaseUrl: `${host}/api/v1`,
        api_versions: ["takosumi.dev/v1alpha1"],
        endpoints: {
          api: `${host}/api/v1`,
          capabilities: `${host}/api/v1/capabilities`,
          openapi: `${host}/openapi.json`,
          oidc_issuer: host,
        },
        shared_account_sign_in: sharedSignInDocument(),
      });
    }
    if (url.origin === selfHost && url.pathname === "/.well-known/takosumi") {
      return Response.json({
        product: "takosumi",
        name: "Takosumi",
        oidcClientId: "takosumi-mobile",
        auth: { oidc: true, password: false },
        apiBaseUrl: `${selfHost}/api/v1`,
        api_versions: ["takosumi.dev/v1alpha1"],
        endpoints: {
          api: `${selfHost}/api/v1`,
          capabilities: `${selfHost}/api/v1/capabilities`,
          openapi: `${selfHost}/openapi.json`,
          oidc_issuer: selfHost,
        },
      });
    }
    if (url.pathname === "/.well-known/openid-configuration") {
      return Response.json({
        issuer: url.origin,
        authorization_endpoint: `${url.origin}/oauth/authorize`,
        token_endpoint: `${url.origin}/oauth/token`,
        revocation_endpoint: `${url.origin}/oauth/revoke`,
      });
    }
    if (url.origin === selfHost && url.pathname === "/oauth/token") {
      return Response.json({ access_token: "self-access", refresh_token: "self-refresh", token_type: "Bearer", expires_in: 300 });
    }
    if (url.origin === selfHost && url.pathname === "/oauth/revoke") return new Response(null, { status: 200 });
    if (url.origin === host && url.pathname === "/api/v1/account/session/mobile-exchange") {
      if (init?.method !== "POST" || headers.has("authorization")) return Response.json({ error: "unauthenticated" }, { status: 401 });
      if (options.exchangeStatus) return Response.json({ error: "sign_in_refused" }, { status: options.exchangeStatus });
      const sent = JSON.parse(body ?? "{}") as { code: string; code_verifier: string; nonce: string };
      const ok = Object.keys(sent).sort().join() === "code,code_verifier,nonce" &&
        sent.code === "code-ok" && sent.nonce === authorize.nonce &&
        (await sha256Base64Url(sent.code_verifier)) === authorize.challenge;
      if (!ok) return Response.json({ error: "sign_in_refused" }, { status: 401 });
      return Response.json(options.exchangeBody ?? {
        token_type: "takosumi-native-session",
        session_token: sessionToken,
        session_header: "x-takosumi-native-session",
        account_id: "tsub_shared",
        expires_at: Math.floor(Date.now() / 1000) + 7 * 24 * 3600,
      });
    }
    if (url.origin === host && url.pathname.startsWith("/api/v1/")) {
      // The shared-account host: no bearer, only the native session header.
      if (headers.has("authorization") || headers.get(TAKOSUMI_NATIVE_SESSION_HEADER) !== sessionToken || revoked) {
        return Response.json({ error: "unauthenticated" }, { status: 401 });
      }
      if (url.pathname === "/api/v1/account/session/me" && init?.method === "DELETE") {
        revoked = true;
        return new Response(null, { status: 204 });
      }
      if (url.pathname === "/api/v1/apps") return Response.json({ apps: [] });
      if (url.pathname === "/api/v1/account/notification-pushers" && init?.method === "POST") {
        return Response.json({
          pusher: { id: "npsh_1", app_id: "com.takosumi", pushkey: "k".repeat(87), data: { url: "https://relay.example/push/1" } },
        }, { status: 201 });
      }
    }
    return new Response(null, { status: 404 });
  };
  return {
    fetch,
    requests,
    noteAuthorize(url: URL) {
      authorize.challenge = url.searchParams.get("code_challenge") ?? undefined;
      authorize.nonce = url.searchParams.get("nonce") ?? undefined;
    },
  };
}

function memoryBridge(): NativeBridge & { readonly opened: string[]; readonly secure: Map<string, string> } {
  const secure = new Map<string, string>();
  const plain = new Map<string, string>();
  const opened: string[] = [];
  return {
    opened,
    secure,
    capabilities: {
      launchPayload: true,
      launchPayloadEvents: false,
      externalBrowser: true,
      inAppBrowser: false,
      qrScanner: false,
      localNotifications: false,
      pushNotifications: false,
      biometricAuth: false,
      callIntent: false,
      clipboardText: false,
      secureStorage: true,
      persistentStorage: true,
    },
    storage: {
      kind: "device-persistent",
      get: async (key) => plain.get(key),
      set: async (key, value) => void plain.set(key, value),
      delete: async (key) => void plain.delete(key),
    },
    secureStore: {
      kind: "secure",
      get: async (key) => secure.get(key),
      set: async (key, value) => void secure.set(key, value),
      delete: async (key) => void secure.delete(key),
    },
    getLaunchPayload: async () => undefined,
    openExternalUrl: async (url) => void opened.push(url),
  };
}

async function discovery(network: ReturnType<typeof fakeNetwork>, hostUrl = host): Promise<HostDiscovery> {
  return await discoverHost({ hostUrl, expectedProduct: productAdapter.discoveryProduct, fetch: network.fetch });
}

test("a shared-account host's sign-in is read only in its exact shape and only on the host itself", async () => {
  const base = await discovery(fakeNetwork());
  expect(readSharedAccountSignIn(base)).toEqual({
    hostUrl: host,
    issuer,
    clientId,
    redirectUri: `${host}/auth/oidc/native/callback`,
    sessionExchange: `${host}/api/v1/account/session/mobile-exchange`,
  });
  const withSignIn = (value: unknown): HostDiscovery => ({
    ...base,
    takosumi: { ...base.takosumi, shared_account_sign_in: value },
    product: undefined,
  });
  for (const overrides of [
    { redirect_uri: "https://evil.example/auth/oidc/native/callback" },
    { redirect_uri: `${host}/elsewhere` },
    { redirect_uri: `http://app.example/auth/oidc/native/callback` },
    { session_exchange: "https://evil.example/api/v1/account/session/mobile-exchange" },
    { session_exchange: `${host}/api/v1/other` },
    { session_header: "authorization" },
    { scope: "openid profile offline_access capsules:read capsules:write" },
    { issuer: `${issuer}/` },
    { issuer: `${issuer}/path` },
    { issuer: "http://auth.example" },
    { client_id: "bad client" },
    { client_id: 7 },
  ]) {
    expect(readSharedAccountSignIn(withSignIn(sharedSignInDocument(overrides))), JSON.stringify(overrides)).toBeUndefined();
  }
  expect(readSharedAccountSignIn(withSignIn(undefined))).toBeUndefined();
  // A self-hosted instance has none and keeps the standard flow.
  expect(readSharedAccountSignIn(await discovery(fakeNetwork(), selfHost))).toBeUndefined();
});

test("signing in to a shared-account host: issuer consent with PKCE and a nonce, the host redeems the code, and the session never travels as a bearer", async () => {
  const network = fakeNetwork();
  const bridge = memoryBridge();
  const controller = createMobileClientController({
    adapter: productAdapter,
    nativeBridge: bridge,
    fetch: network.fetch,
    customSignIn: createSharedAccountSignIn(),
  });
  await controller.start();
  await controller.connectWithInput("app.example");
  await controller.startSignIn();

  const authorize = new URL(bridge.opened.at(-1)!);
  network.noteAuthorize(authorize);
  expect(`${authorize.origin}${authorize.pathname}`).toBe(`${issuer}/oauth/authorize`);
  expect(Object.fromEntries(authorize.searchParams)).toMatchObject({
    response_type: "code",
    client_id: clientId,
    redirect_uri: `${host}/auth/oidc/native/callback`,
    scope: "openid email offline_access",
    code_challenge_method: "S256",
    prompt: "consent",
  });
  expect(authorize.searchParams.get("nonce")!.length).toBeGreaterThanOrEqual(43);
  expect(authorize.searchParams.get("code_challenge")).toBeTruthy();
  const state = authorize.searchParams.get("state")!;
  expect(bridge.secure.has(SHARED_SIGN_IN_PENDING_KEY)).toBe(true);

  // The host's redirect page hands the response to the app.
  await controller.handleLaunchPayload(`takosumi://oauth/callback?code=code-ok&state=${state}&iss=${encodeURIComponent(issuer)}`);
  const session = controller.getState().session!;
  expect(session).toMatchObject({
    hostUrl: host,
    product: "takosumi",
    oidcIssuer: issuer,
    oidcClientId: clientId,
    accountId: "tsub_shared",
    accessToken: sessionToken,
    tokenType: "takosumi-native-session",
  });
  expect(session.refreshToken).toBeUndefined();
  expect(bridge.secure.has(SHARED_SIGN_IN_PENDING_KEY)).toBe(false);

  // The app never redeemed the code itself.
  expect(network.requests.some((r) => r.url.endsWith("/oauth/token"))).toBe(false);

  // The management API takes the session in its own header.
  globalThis.fetch = network.fetch as typeof fetch;
  expect(await loadHome(session)).toMatchObject({ apps: [], runs: [] });
  const pusher = await registerPusher(session, network.fetch, {
    pushkey: "k".repeat(87), endpoint: "https://relay.example/push/1", auth: "a".repeat(22),
    deviceDisplayName: "Pixel 9", lang: "ja-JP",
  });
  expect(pusher.id).toBe("npsh_1");

  await controller.signOut();
  const signOut = network.requests.find((r) => r.method === "DELETE" && r.url === `${host}/api/v1/account/session/me`);
  expect(signOut?.headers.get(TAKOSUMI_NATIVE_SESSION_HEADER)).toBe(sessionToken);
  expect(controller.getState().session).toBeUndefined();

  // The session secret only ever went to the host, and never as a bearer.
  for (const request of network.requests) {
    expect(request.headers.get("authorization") ?? "").not.toContain(sessionToken);
    if (request.headers.get(TAKOSUMI_NATIVE_SESSION_HEADER) || request.body?.includes(sessionToken)) {
      expect(new URL(request.url).origin).toBe(host);
    }
  }
  controller.stop();
});

test("a refused, malformed or stale shared sign-in leaves no session", async () => {
  for (const scenario of [
    { name: "refused", options: { exchangeStatus: 401 }, callback: (state: string) => `takosumi://oauth/callback?code=code-ok&state=${state}` },
    { name: "rate limited", options: { exchangeStatus: 429 }, callback: (state: string) => `takosumi://oauth/callback?code=code-ok&state=${state}` },
    { name: "malformed answer", options: { exchangeBody: { token_type: "Bearer", session_token: sessionToken } }, callback: (state: string) => `takosumi://oauth/callback?code=code-ok&state=${state}` },
    { name: "wrong code", options: {}, callback: (state: string) => `takosumi://oauth/callback?code=code-stolen&state=${state}` },
    { name: "other issuer", options: {}, callback: (state: string) => `takosumi://oauth/callback?code=code-ok&state=${state}&iss=${encodeURIComponent("https://evil.example")}` },
    { name: "issuer error", options: {}, callback: (state: string) => `takosumi://oauth/callback?error=access_denied&state=${state}` },
  ]) {
    const network = fakeNetwork(scenario.options);
    const bridge = memoryBridge();
    const controller = createMobileClientController({
      adapter: productAdapter, nativeBridge: bridge, fetch: network.fetch, customSignIn: createSharedAccountSignIn(),
    });
    await controller.start();
    await controller.connectWithInput("app.example");
    await controller.startSignIn();
    const authorize = new URL(bridge.opened.at(-1)!);
    network.noteAuthorize(authorize);
    await controller.handleLaunchPayload(scenario.callback(authorize.searchParams.get("state")!));
    expect(controller.getState().session, scenario.name).toBeUndefined();
    expect(controller.getState().accounts, scenario.name).toHaveLength(0);
    // The attempt is spent either way.
    expect(bridge.secure.has(SHARED_SIGN_IN_PENDING_KEY), scenario.name).toBe(false);
    controller.stop();
  }

  // An expired attempt is refused before any exchange.
  let clock = Date.parse("2026-10-30T00:00:00.000Z");
  const network = fakeNetwork();
  const bridge = memoryBridge();
  const hook = createSharedAccountSignIn({ now: () => new Date(clock) });
  const { authorizationUrl } = await hook.begin({ discovery: await discovery(network), nativeBridge: bridge, fetch: network.fetch });
  clock += 11 * 60 * 1000;
  const state = new URL(authorizationUrl).searchParams.get("state");
  await expect(hook.complete({ callbackUrl: `takosumi://oauth/callback?code=code-ok&state=${state}`, nativeBridge: bridge, fetch: network.fetch }))
    .rejects.toThrow("有効期限");
  expect(network.requests.some((r) => r.url.includes("mobile-exchange"))).toBe(false);
});

test("a callback that is not the shared sign-in's own falls through to mobile-kit, and self-hosted instances keep the standard flow", async () => {
  const network = fakeNetwork();
  const bridge = memoryBridge();
  const hook = createSharedAccountSignIn();
  expect(await hook.complete({ callbackUrl: "takosumi://oauth/callback?code=c&state=unknown", nativeBridge: bridge })).toBeUndefined();

  const controller = createMobileClientController({
    adapter: productAdapter, nativeBridge: bridge, fetch: network.fetch, customSignIn: hook,
  });
  await controller.start();
  await controller.connectWithInput("operator.example");
  await controller.startSignIn();
  const authorize = new URL(bridge.opened.at(-1)!);
  expect(authorize.origin).toBe(selfHost);
  expect(authorize.searchParams.get("client_id")).toBe("takosumi-mobile");
  expect(authorize.searchParams.get("redirect_uri")).toBe("takosumi://oauth/callback");
  expect(bridge.secure.has(SHARED_SIGN_IN_PENDING_KEY)).toBe(false);
  await controller.completeSignIn(
    `takosumi://oauth/callback?code=code&state=${authorize.searchParams.get("state")}&iss=${encodeURIComponent(selfHost)}`,
  );
  const session = controller.getState().session as MobileSession;
  expect(session.tokenType).toBe("Bearer");
  expect(takosumiSessionFetch(session)).toBeUndefined();
  controller.stop();
});

test("the native session fetch replaces Authorization with the session header", async () => {
  const seen: Headers[] = [];
  const fetcher: FetchLike = async (_input, init) => {
    seen.push(new Headers(init?.headers));
    return new Response(null, { status: 204 });
  };
  const wrapped = takosumiSessionFetch({ tokenType: "takosumi-native-session", accessToken: sessionToken }, fetcher)!;
  await wrapped(`${host}/api/v1/apps`, { headers: { authorization: `takosumi-native-session ${sessionToken}`, accept: "application/json" } });
  expect(seen[0]!.get("authorization")).toBeNull();
  expect(seen[0]!.get(TAKOSUMI_NATIVE_SESSION_HEADER)).toBe(sessionToken);
  expect(seen[0]!.get("accept")).toBe("application/json");
});
