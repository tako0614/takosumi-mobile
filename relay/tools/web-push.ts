// A minimal standards Web Push sender and receiver: RFC 8291 (aes128gcm
// message encryption) and RFC 8292 (VAPID). The relay itself never encrypts or
// decrypts; this exists so tests and the staging smoke send exactly what a real
// application server sends, and so the receiving side's math is pinned by the
// RFC 8291 Appendix A test vector before the native code is written.

import { base64UrlEncode, concatBytes, toArrayBuffer, utf8 } from "../src/encoding.ts";
import { signEs256 } from "../src/jwt.ts";

async function hmac(key: Uint8Array, data: Uint8Array): Promise<Uint8Array> {
  const imported = await crypto.subtle.importKey(
    "raw",
    toArrayBuffer(key),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  return new Uint8Array(await crypto.subtle.sign("HMAC", imported, toArrayBuffer(data)));
}

async function importEcdhPublic(raw: Uint8Array): Promise<CryptoKey> {
  return crypto.subtle.importKey("raw", toArrayBuffer(raw), { name: "ECDH", namedCurve: "P-256" }, true, []);
}

async function exportRaw(key: CryptoKey): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.exportKey("raw", key));
}

/** Imports a P-256 private key given as raw `d` plus its uncompressed public key. */
export async function importP256Private(
  d: Uint8Array,
  publicKey: Uint8Array,
  algorithm: "ECDH" | "ECDSA",
): Promise<CryptoKey> {
  return crypto.subtle.importKey(
    "jwk",
    {
      kty: "EC",
      crv: "P-256",
      d: base64UrlEncode(d),
      x: base64UrlEncode(publicKey.slice(1, 33)),
      y: base64UrlEncode(publicKey.slice(33, 65)),
      ext: true,
    },
    { name: algorithm, namedCurve: "P-256" },
    true,
    algorithm === "ECDH" ? ["deriveBits"] : ["sign"],
  );
}

async function deriveKeys(
  sharedSecret: Uint8Array,
  authSecret: Uint8Array,
  uaPublic: Uint8Array,
  asPublic: Uint8Array,
  salt: Uint8Array,
): Promise<{ cek: Uint8Array; nonce: Uint8Array }> {
  const prkKey = await hmac(authSecret, sharedSecret);
  const keyInfo = concatBytes(utf8("WebPush: info\0"), uaPublic, asPublic);
  const ikm = await hmac(prkKey, concatBytes(keyInfo, new Uint8Array([1])));
  const prk = await hmac(salt, ikm);
  const cek = (await hmac(prk, concatBytes(utf8("Content-Encoding: aes128gcm\0"), new Uint8Array([1])))).slice(0, 16);
  const nonce = (await hmac(prk, concatBytes(utf8("Content-Encoding: nonce\0"), new Uint8Array([1])))).slice(0, 12);
  return { cek, nonce };
}

export interface SenderKeys {
  publicKey: Uint8Array;
  privateKey: CryptoKey;
}

/** Encrypts one Web Push message (RFC 8291) as a single aes128gcm record. */
export async function encryptWebPush(options: {
  uaPublic: Uint8Array;
  authSecret: Uint8Array;
  plaintext: Uint8Array;
  salt?: Uint8Array;
  senderKeys?: SenderKeys;
  recordSize?: number;
}): Promise<Uint8Array> {
  const salt = options.salt ?? crypto.getRandomValues(new Uint8Array(16));
  let sender = options.senderKeys;
  if (!sender) {
    const pair = (await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, true, [
      "deriveBits",
    ])) as CryptoKeyPair;
    sender = { publicKey: await exportRaw(pair.publicKey), privateKey: pair.privateKey };
  }
  const shared = new Uint8Array(
    await crypto.subtle.deriveBits(
      { name: "ECDH", public: await importEcdhPublic(options.uaPublic) },
      sender.privateKey,
      256,
    ),
  );
  const { cek, nonce } = await deriveKeys(shared, options.authSecret, options.uaPublic, sender.publicKey, salt);
  const key = await crypto.subtle.importKey("raw", toArrayBuffer(cek), "AES-GCM", false, ["encrypt"]);
  const padded = concatBytes(options.plaintext, new Uint8Array([2]));
  const ciphertext = new Uint8Array(
    await crypto.subtle.encrypt({ name: "AES-GCM", iv: toArrayBuffer(nonce) }, key, toArrayBuffer(padded)),
  );
  const recordSize = options.recordSize ?? 4096;
  const header = new Uint8Array(21);
  header.set(salt, 0);
  new DataView(header.buffer).setUint32(16, recordSize);
  header[20] = sender.publicKey.length;
  return concatBytes(header, sender.publicKey, ciphertext);
}

/** Decrypts one Web Push message, as the device will. */
export async function decryptWebPush(options: {
  uaPrivate: CryptoKey;
  uaPublic: Uint8Array;
  authSecret: Uint8Array;
  body: Uint8Array;
}): Promise<Uint8Array> {
  const { body } = options;
  const salt = body.slice(0, 16);
  const keyIdLength = body[20] ?? 0;
  const asPublic = body.slice(21, 21 + keyIdLength);
  const ciphertext = body.slice(21 + keyIdLength);
  const shared = new Uint8Array(
    await crypto.subtle.deriveBits({ name: "ECDH", public: await importEcdhPublic(asPublic) }, options.uaPrivate, 256),
  );
  const { cek, nonce } = await deriveKeys(shared, options.authSecret, options.uaPublic, asPublic, salt);
  const key = await crypto.subtle.importKey("raw", toArrayBuffer(cek), "AES-GCM", false, ["decrypt"]);
  const padded = new Uint8Array(
    await crypto.subtle.decrypt({ name: "AES-GCM", iv: toArrayBuffer(nonce) }, key, toArrayBuffer(ciphertext)),
  );
  let end = padded.length - 1;
  while (end >= 0 && padded[end] === 0) end -= 1;
  if (end < 0 || padded[end] !== 2) throw new Error("missing last-record delimiter");
  return padded.slice(0, end);
}

export interface VapidKeys {
  publicKey: Uint8Array;
  privateKey: CryptoKey;
}

export async function generateVapidKeys(): Promise<VapidKeys> {
  const pair = (await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, [
    "sign",
    "verify",
  ])) as CryptoKeyPair;
  return { publicKey: await exportRaw(pair.publicKey), privateKey: pair.privateKey };
}

/** `vapid t=<JWT>, k=<key>` for one push resource origin. */
export async function vapidAuthorization(
  keys: VapidKeys,
  audience: string,
  options: { subject?: string; expiresAt: number; header?: Record<string, unknown> },
): Promise<string> {
  const claims: Record<string, unknown> = { aud: audience, exp: options.expiresAt };
  if (options.subject) claims.sub = options.subject;
  const token = await signEs256(options.header ?? { typ: "JWT", alg: "ES256" }, claims, keys.privateKey);
  return `vapid t=${token}, k=${base64UrlEncode(keys.publicKey)}`;
}

export interface UserAgentKeys {
  publicKey: Uint8Array;
  privateKey: CryptoKey;
  authSecret: Uint8Array;
}

/** What a browser (or Takosumi Mobile) creates for one subscription. */
export async function generateUserAgentKeys(): Promise<UserAgentKeys> {
  const pair = (await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, true, [
    "deriveBits",
  ])) as CryptoKeyPair;
  return {
    publicKey: await exportRaw(pair.publicKey),
    privateKey: pair.privateKey,
    authSecret: crypto.getRandomValues(new Uint8Array(16)),
  };
}

/** Sends one Web Push message the way an application server does. */
export async function sendWebPush(options: {
  endpoint: string;
  vapid: VapidKeys;
  userAgent: Pick<UserAgentKeys, "publicKey" | "authSecret">;
  payload: Uint8Array;
  ttl: number;
  urgency?: string;
  topic?: string;
  subject?: string;
  now?: number;
  fetcher?: (input: string, init?: RequestInit) => Promise<Response>;
}): Promise<Response> {
  const now = options.now ?? Math.floor(Date.now() / 1000);
  const body = await encryptWebPush({
    uaPublic: options.userAgent.publicKey,
    authSecret: options.userAgent.authSecret,
    plaintext: options.payload,
  });
  const headers: Record<string, string> = {
    authorization: await vapidAuthorization(options.vapid, new URL(options.endpoint).origin, {
      subject: options.subject ?? "mailto:relay-smoke@example.invalid",
      expiresAt: now + 12 * 3600,
    }),
    "content-encoding": "aes128gcm",
    "content-type": "application/octet-stream",
    ttl: String(options.ttl),
  };
  if (options.urgency) headers.urgency = options.urgency;
  if (options.topic) headers.topic = options.topic;
  const fetcher = options.fetcher ?? ((input, init) => fetch(input, init));
  return fetcher(options.endpoint, { method: "POST", headers, body: toArrayBuffer(body), redirect: "manual" });
}
