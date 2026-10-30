/**
 * Takosumi Mobile's sign-in on a shared-account host (Hosted, app.takosumi.com;
 * ADR 0025): people there sign in only through the host's private relying
 * party (RP), and the host accepts no PAT or OAuth bearer. The app signs in
 * at the same issuer the browser uses, as the Takosumi Mobile client, and
 * receives an RP session of the browser's kind (takosumi
 * `docs/reference/api.md`「共有アカウントの host での Takosumi Mobile のサインイン」):
 *
 * 1. authorization code with PKCE (S256), `state`, `nonce` and
 *    `prompt=consent` at the advertised issuer, with the host's HTTPS
 *    redirect page, which hands the response to `takosumi://oauth/callback`;
 * 2. the app does not redeem the code: it posts `{code, code_verifier, nonce}`
 *    to the host, whose RP redeems it with the client secret only it holds
 *    and verifies the ID token;
 * 3. the app presents the returned session in `x-takosumi-native-session`.
 *
 * Self-hosted instances (with `oidcClientId`) keep mobile-kit's own flow.
 */

import {
  createPkcePair,
  createRandomState,
  fetchOidcMetadata,
  normalizeHostUrl,
  parseOidcCallback,
  resolveMobileApiUrl,
  sameOriginApiBaseUrl,
  type FetchLike,
  type HostDiscovery,
  type MobileCustomSignIn,
  type MobileSession,
  type NativeBridge,
} from "@takosjp/mobile-kit";

export const TAKOSUMI_NATIVE_SESSION_TYPE = "takosumi-native-session";
export const TAKOSUMI_NATIVE_SESSION_HEADER = "x-takosumi-native-session";
export const SHARED_SIGN_IN_SCOPE = "openid email offline_access";
export const SHARED_SIGN_IN_PENDING_KEY = "takosumi.mobile.takosumi.shared-sign-in.pending";
const SHARED_SIGN_IN_CALLBACK_PATH = "/auth/oidc/native/callback";
const SHARED_SIGN_IN_EXCHANGE_PATH = "/api/v1/account/session/mobile-exchange";
const APP_REDIRECT = "takosumi://oauth/callback";
const PENDING_TTL_MS = 10 * 60 * 1000;
const CLIENT_ID = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/u;
const SESSION_TOKEN = /^[A-Za-z0-9_-]{32,256}$/u;
const ACCOUNT_ID = /^tsub_[A-Za-z0-9_-]{1,124}$/u;
const MAX_SESSION_SECONDS = 8 * 24 * 60 * 60;

/** What a shared-account host advertises in `shared_account_sign_in`. */
export interface SharedAccountSignIn {
  readonly hostUrl: string;
  readonly issuer: string;
  readonly clientId: string;
  readonly redirectUri: string;
  readonly sessionExchange: string;
}

interface PendingSharedSignIn extends SharedAccountSignIn {
  readonly apiBaseUrl?: string;
  readonly state: string;
  readonly nonce: string;
  readonly codeVerifier: string;
  readonly createdAt: string;
}

function record(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

function httpsUrl(value: unknown): URL | undefined {
  if (typeof value !== "string") return undefined;
  try {
    const url = new URL(value);
    return url.protocol === "https:" && !url.username && !url.password && !url.hash ? url : undefined;
  } catch {
    return undefined;
  }
}

/**
 * Reads the host's shared-account sign-in, or undefined for any host that does
 * not offer it in exactly the expected shape. The redirect page and the
 * exchange must be this host's own fixed paths: a host can never send the
 * code, verifier or nonce anywhere else, and the issuer only redirects to a
 * URI registered for the client, so a host naming another host's issuer and
 * client cannot receive that host's authorization response.
 */
export function readSharedAccountSignIn(discovery: HostDiscovery): SharedAccountSignIn | undefined {
  const document = record(discovery.takosumi) ?? record(discovery.product);
  const value = record(document?.shared_account_sign_in);
  if (!value) return undefined;
  let host: string;
  try {
    host = new URL(normalizeHostUrl(discovery.hostUrl)).origin;
  } catch {
    return undefined;
  }
  const issuer = httpsUrl(value.issuer);
  const redirect = httpsUrl(value.redirect_uri);
  const exchange = httpsUrl(value.session_exchange);
  if (
    !issuer || issuer.href !== `${issuer.origin}/` || issuer.search || value.issuer !== issuer.origin ||
    typeof value.client_id !== "string" || !CLIENT_ID.test(value.client_id) ||
    !redirect || redirect.href !== `${host}${SHARED_SIGN_IN_CALLBACK_PATH}` ||
    !exchange || exchange.href !== `${host}${SHARED_SIGN_IN_EXCHANGE_PATH}` ||
    value.session_header !== TAKOSUMI_NATIVE_SESSION_HEADER ||
    value.scope !== SHARED_SIGN_IN_SCOPE
  ) {
    return undefined;
  }
  return {
    hostUrl: host,
    issuer: issuer.origin,
    clientId: value.client_id,
    redirectUri: redirect.href,
    sessionExchange: exchange.href,
  };
}

/** True for a session from this sign-in (presented in its header, never as a bearer). */
export function isTakosumiNativeSession(session: Pick<MobileSession, "tokenType">): boolean {
  return session.tokenType === TAKOSUMI_NATIVE_SESSION_TYPE;
}

/** The credential header a request for this session carries. */
export function takosumiSessionHeaders(
  session: Pick<MobileSession, "tokenType" | "accessToken">,
): Record<string, string> {
  return isTakosumiNativeSession(session)
    ? { [TAKOSUMI_NATIVE_SESSION_HEADER]: session.accessToken }
    : { authorization: `${session.tokenType} ${session.accessToken}` };
}

/**
 * A fetch for mobile-kit's API client that sends a native session in its own
 * header instead of `Authorization`, or undefined for any other session (the
 * client's default then applies unchanged). The API client only builds URLs
 * on the session's own host.
 */
export function takosumiSessionFetch(
  session: Pick<MobileSession, "tokenType" | "accessToken">,
  fetcher?: FetchLike,
): FetchLike | undefined {
  if (!isTakosumiNativeSession(session)) return undefined;
  return async (input, init) => {
    const headers = new Headers(init?.headers);
    headers.delete("authorization");
    headers.set(TAKOSUMI_NATIVE_SESSION_HEADER, session.accessToken);
    return await (fetcher ?? globalThis.fetch.bind(globalThis))(input, { ...init, headers });
  };
}

function randomNonce(cryptoSource: Crypto): string {
  const bytes = cryptoSource.getRandomValues(new Uint8Array(32));
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/gu, "-").replace(/\//gu, "_").replace(/=+$/u, "");
}

function secureStore(bridge: NativeBridge) {
  const store = bridge.secureStore;
  if (!store) throw new Error("この端末では安全な保存先が使えないため、サインインできません。");
  return store;
}

async function readPending(bridge: NativeBridge): Promise<PendingSharedSignIn | undefined> {
  const raw = await bridge.secureStore?.get(SHARED_SIGN_IN_PENDING_KEY);
  if (!raw) return undefined;
  try {
    const value = record(JSON.parse(raw));
    if (
      !value ||
      ["hostUrl", "issuer", "clientId", "redirectUri", "sessionExchange", "state", "nonce", "codeVerifier", "createdAt"]
        .some((key) => typeof value[key] !== "string") ||
      (value.apiBaseUrl !== undefined && typeof value.apiBaseUrl !== "string")
    ) {
      return undefined;
    }
    return value as unknown as PendingSharedSignIn;
  } catch {
    return undefined;
  }
}

function exchangeFailure(status: number): Error {
  if (status === 401) {
    return new Error("Takosumiがサインインを受け付けませんでした。もう一度サインインしてください。");
  }
  if (status === 429) {
    return new Error("サインインの試行が多すぎます。少し待ってからもう一度お試しください。");
  }
  return new Error(`Takosumiにサインインできませんでした (${status})。`);
}

async function exchangeCode(input: {
  readonly pending: PendingSharedSignIn;
  readonly code: string;
  readonly fetch?: FetchLike;
  readonly now: Date;
}): Promise<MobileSession> {
  const fetcher = input.fetch ?? globalThis.fetch.bind(globalThis);
  const response = await fetcher(input.pending.sessionExchange, {
    method: "POST",
    headers: { accept: "application/json", "content-type": "application/json" },
    body: JSON.stringify({
      code: input.code,
      code_verifier: input.pending.codeVerifier,
      nonce: input.pending.nonce,
    }),
  });
  if (!response.ok) throw exchangeFailure(response.status);
  const body = record(await response.json().catch(() => undefined));
  const nowSeconds = Math.floor(input.now.getTime() / 1000);
  if (
    !body ||
    body.token_type !== TAKOSUMI_NATIVE_SESSION_TYPE ||
    body.session_header !== TAKOSUMI_NATIVE_SESSION_HEADER ||
    typeof body.session_token !== "string" || !SESSION_TOKEN.test(body.session_token) ||
    typeof body.account_id !== "string" || !ACCOUNT_ID.test(body.account_id) ||
    typeof body.expires_at !== "number" || !Number.isSafeInteger(body.expires_at) ||
    body.expires_at <= nowSeconds || body.expires_at > nowSeconds + MAX_SESSION_SECONDS
  ) {
    throw new Error("Takosumiが不正なサインインの応答を返しました。");
  }
  return {
    hostUrl: input.pending.hostUrl,
    product: "takosumi",
    oidcIssuer: input.pending.issuer,
    oidcClientId: input.pending.clientId,
    ...(input.pending.apiBaseUrl ? { apiBaseUrl: input.pending.apiBaseUrl } : {}),
    accountId: body.account_id,
    // mobile-kit reads this as "a host-exchanged session": it never sends this
    // session's credential to the issuer's revocation endpoint.
    productEndpoints: { mobileOidcExchange: input.pending.sessionExchange },
    accessToken: body.session_token,
    tokenType: TAKOSUMI_NATIVE_SESSION_TYPE,
    createdAt: input.now.toISOString(),
    // The RP session's absolute expiry. There is no refresh token, so near
    // this time mobile-kit ends the session and asks for a new sign-in.
    expiresAt: new Date(body.expires_at * 1000).toISOString(),
  };
}

export interface SharedAccountSignInOptions {
  readonly now?: () => Date;
  readonly crypto?: Crypto;
}

/** The mobile-kit sign-in hook for shared-account hosts. */
export function createSharedAccountSignIn(options: SharedAccountSignInOptions = {}): MobileCustomSignIn {
  const now = () => options.now?.() ?? new Date();
  const cryptoSource = options.crypto ?? globalThis.crypto;
  return {
    handles: (discovery) => readSharedAccountSignIn(discovery) !== undefined,
    async begin({ discovery, nativeBridge, fetch }) {
      const config = readSharedAccountSignIn(discovery);
      if (!config) throw new Error("このTakosumiは共有アカウントのサインインを提供していません。");
      const store = secureStore(nativeBridge);
      // The issuer must serve its own authorize endpoint (mix-up defence).
      const metadata = await fetchOidcMetadata({ issuer: config.issuer, fetch });
      const pkce = await createPkcePair(cryptoSource);
      const state = createRandomState(cryptoSource);
      const nonce = randomNonce(cryptoSource);
      const apiBaseUrl = sameOriginApiBaseUrl(discovery.apiBaseUrl, discovery.hostUrl);
      const pending: PendingSharedSignIn = {
        ...config,
        ...(apiBaseUrl ? { apiBaseUrl } : {}),
        state,
        nonce,
        codeVerifier: pkce.codeVerifier,
        createdAt: now().toISOString(),
      };
      await store.set(SHARED_SIGN_IN_PENDING_KEY, JSON.stringify(pending));
      const url = new URL(metadata.authorization_endpoint);
      url.searchParams.set("response_type", "code");
      url.searchParams.set("client_id", config.clientId);
      url.searchParams.set("redirect_uri", config.redirectUri);
      url.searchParams.set("scope", SHARED_SIGN_IN_SCOPE);
      url.searchParams.set("state", state);
      url.searchParams.set("nonce", nonce);
      url.searchParams.set("code_challenge", pkce.codeChallenge);
      url.searchParams.set("code_challenge_method", "S256");
      // Another app can claim the custom scheme; an explicit consent each time
      // keeps a silent sign-in from reaching it (RFC 8252 §8.6).
      url.searchParams.set("prompt", "consent");
      return { authorizationUrl: url.toString() };
    },
    async complete({ callbackUrl, nativeBridge, fetch }) {
      const pending = await readPending(nativeBridge);
      if (!pending) return undefined;
      let callbackState: string | null;
      try {
        callbackState = new URL(callbackUrl).searchParams.get("state");
      } catch {
        return undefined;
      }
      // Not this sign-in's callback: mobile-kit's own flow may own it.
      if (callbackState !== pending.state) return undefined;
      // One attempt per sign-in: the verifier and nonce are gone after this.
      await nativeBridge.secureStore?.delete(SHARED_SIGN_IN_PENDING_KEY);
      const createdAt = Date.parse(pending.createdAt);
      if (!Number.isFinite(createdAt) || now().getTime() - createdAt > PENDING_TTL_MS) {
        throw new Error("サインインの有効期限が切れました。もう一度サインインしてください。");
      }
      const callback = parseOidcCallback(callbackUrl, pending.state, APP_REDIRECT, pending.issuer, false);
      return await exchangeCode({ pending, code: callback.code, fetch, now: now() });
    },
    async revokeSession({ session, fetch }) {
      if (!isTakosumiNativeSession(session)) return false;
      const fetcher = fetch ?? globalThis.fetch.bind(globalThis);
      try {
        await fetcher(resolveMobileApiUrl(session, "/account/session/me", "/api/v1"), {
          method: "DELETE",
          headers: { accept: "application/json", ...takosumiSessionHeaders(session) },
        });
      } catch {
        // Best effort: the RP's idle and absolute limits still end it.
      }
      return true;
    },
  };
}
