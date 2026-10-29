// VAPID (RFC 8292) verification for the push resource.
//
// A request carries `Authorization: vapid t=<JWT>, k=<public key>`. The relay
// accepts it only when the JWT is ES256-signed by `k`, `k` is the application
// server key the subscription was created with, `aud` is the relay's origin and
// `exp` is in the future but no more than 24 hours away. Someone who learned an
// endpoint URL but does not hold the application server's private key cannot
// send to it.
//
// The JWT's `sub` claim is never read, so it can never reach a log.

import {
  base64UrlDecode,
  constantTimeEqual,
  fromUtf8,
  toArrayBuffer,
  utf8,
} from "./encoding.ts";

export const MAX_VAPID_LIFETIME_SECONDS = 24 * 60 * 60;
const CLOCK_SKEW_SECONDS = 60;

export interface VapidFailure {
  status: 401 | 403;
  code: string;
  message: string;
}

export interface VapidCredentials {
  token: string;
  key: string;
}

/** Parses `vapid t=..., k=...`. Returns null when absent or malformed. */
export function parseVapidAuthorization(header: string | null): VapidCredentials | null {
  if (!header) return null;
  const match = header.match(/^\s*vapid\s+(.+)$/iu);
  if (!match?.[1]) return null;
  const params = new Map<string, string>();
  for (const part of match[1].split(",")) {
    const separator = part.indexOf("=");
    if (separator <= 0) return null;
    const name = part.slice(0, separator).trim().toLowerCase();
    let value = part.slice(separator + 1).trim();
    if (value.startsWith('"') && value.endsWith('"') && value.length >= 2) {
      value = value.slice(1, -1);
    }
    if (params.has(name)) return null;
    params.set(name, value);
  }
  const token = params.get("t");
  const key = params.get("k");
  if (!token || !key) return null;
  return { token, key };
}

/**
 * Validates a raw uncompressed P-256 public key (65 bytes, leading 0x04) and
 * returns it as a verification key, or null when it is not a point on the curve.
 */
export async function importApplicationServerKey(raw: Uint8Array): Promise<CryptoKey | null> {
  if (raw.length !== 65 || raw[0] !== 0x04) return null;
  try {
    return await crypto.subtle.importKey(
      "raw",
      toArrayBuffer(raw),
      { name: "ECDSA", namedCurve: "P-256" },
      false,
      ["verify"],
    );
  } catch {
    return null;
  }
}

function parseJsonSegment(segment: string): Record<string, unknown> | null {
  const bytes = base64UrlDecode(segment);
  if (!bytes) return null;
  const text = fromUtf8(bytes);
  if (text === null) return null;
  try {
    const value = JSON.parse(text);
    return value && typeof value === "object" && !Array.isArray(value) ? value : null;
  } catch {
    return null;
  }
}

function sameOrigin(audience: unknown, origin: string): boolean {
  if (typeof audience !== "string") return false;
  const trimmed = audience.endsWith("/") ? audience.slice(0, -1) : audience;
  return trimmed === origin;
}

export async function verifyVapid(
  authorization: string | null,
  expected: {
    /** The relay's origin, for example `https://push.example.com`. */
    audience: string;
    /** The subscription's application server key (raw, 65 bytes). */
    applicationServerKey: Uint8Array;
    /** Unix seconds. */
    now: number;
  },
): Promise<{ ok: true } | { ok: false; failure: VapidFailure }> {
  const fail = (status: 401 | 403, code: string, message: string) => ({
    ok: false as const,
    failure: { status, code, message },
  });

  if (!authorization) {
    return fail(401, "vapid_required", "Authorization: vapid t=..., k=... is required");
  }
  const credentials = parseVapidAuthorization(authorization);
  if (!credentials) {
    return fail(401, "vapid_malformed", "the Authorization header is not a vapid credential");
  }
  const keyBytes = base64UrlDecode(credentials.key);
  if (!keyBytes) return fail(401, "vapid_malformed", "k is not base64url");
  const key = await importApplicationServerKey(keyBytes);
  if (!key) return fail(401, "vapid_malformed", "k is not an uncompressed P-256 public key");

  const segments = credentials.token.split(".");
  if (segments.length !== 3) return fail(401, "vapid_malformed", "t is not a compact JWS");
  const [headerSegment = "", claimsSegment = "", signatureSegment = ""] = segments;
  const header = parseJsonSegment(headerSegment);
  const claims = parseJsonSegment(claimsSegment);
  const signature = base64UrlDecode(signatureSegment);
  if (!header || !claims || !signature) {
    return fail(401, "vapid_malformed", "t is not a valid JWT");
  }
  if (header.alg !== "ES256") return fail(401, "vapid_malformed", "the JWT must use ES256");
  if (header.typ !== undefined && String(header.typ).toUpperCase() !== "JWT") {
    return fail(401, "vapid_malformed", "the JWT typ must be JWT");
  }

  // The key must be the one the subscription was created with (RFC 8292 §4.2).
  if (!constantTimeEqual(keyBytes, expected.applicationServerKey)) {
    return fail(403, "vapid_key_mismatch", "k is not this subscription's application server key");
  }

  const valid =
    signature.length === 64 &&
    (await crypto.subtle.verify(
      { name: "ECDSA", hash: "SHA-256" },
      key,
      toArrayBuffer(signature),
      toArrayBuffer(utf8(`${headerSegment}.${claimsSegment}`)),
    ));
  if (!valid) return fail(403, "vapid_signature_invalid", "the JWT signature does not verify with k");

  if (!sameOrigin(claims.aud, expected.audience)) {
    return fail(403, "vapid_audience_mismatch", "aud must be the push service origin");
  }
  const exp = claims.exp;
  if (typeof exp !== "number" || !Number.isFinite(exp)) {
    return fail(403, "vapid_expiry_missing", "exp is required");
  }
  if (exp <= expected.now - CLOCK_SKEW_SECONDS) {
    return fail(403, "vapid_expired", "the JWT has expired");
  }
  if (exp > expected.now + MAX_VAPID_LIFETIME_SECONDS + CLOCK_SKEW_SECONDS) {
    return fail(403, "vapid_expiry_too_far", "exp must be no more than 24 hours away");
  }
  return { ok: true };
}
