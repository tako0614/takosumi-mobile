import { base64UrlEncode, pemToDer, toArrayBuffer, utf8 } from "./encoding.ts";

/** Signs a compact JWS with an ES256 key (WebCrypto already emits raw r||s). */
export async function signEs256(
  header: Record<string, unknown>,
  claims: Record<string, unknown>,
  key: CryptoKey,
): Promise<string> {
  const signingInput = `${base64UrlEncode(utf8(JSON.stringify(header)))}.${base64UrlEncode(utf8(JSON.stringify(claims)))}`;
  const signature = await crypto.subtle.sign(
    { name: "ECDSA", hash: "SHA-256" },
    key,
    toArrayBuffer(utf8(signingInput)),
  );
  return `${signingInput}.${base64UrlEncode(new Uint8Array(signature))}`;
}

/** Signs a compact JWS with an RS256 key. */
export async function signRs256(
  header: Record<string, unknown>,
  claims: Record<string, unknown>,
  key: CryptoKey,
): Promise<string> {
  const signingInput = `${base64UrlEncode(utf8(JSON.stringify(header)))}.${base64UrlEncode(utf8(JSON.stringify(claims)))}`;
  const signature = await crypto.subtle.sign(
    { name: "RSASSA-PKCS1-v1_5" },
    key,
    toArrayBuffer(utf8(signingInput)),
  );
  return `${signingInput}.${base64UrlEncode(new Uint8Array(signature))}`;
}

/** Imports an APNs `.p8` key (PKCS#8 P-256) for ES256 signing. */
export async function importEs256PrivateKey(pem: string): Promise<CryptoKey> {
  const der = pemToDer(pem, "PRIVATE KEY");
  if (!der) throw new Error("not a PKCS#8 PEM private key");
  return crypto.subtle.importKey(
    "pkcs8",
    toArrayBuffer(der),
    { name: "ECDSA", namedCurve: "P-256" },
    false,
    ["sign"],
  );
}

/** Imports a Google service account key (PKCS#8 RSA) for RS256 signing. */
export async function importRs256PrivateKey(pem: string): Promise<CryptoKey> {
  const der = pemToDer(pem, "PRIVATE KEY");
  if (!der) throw new Error("not a PKCS#8 PEM private key");
  return crypto.subtle.importKey(
    "pkcs8",
    toArrayBuffer(der),
    { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" },
    false,
    ["sign"],
  );
}
