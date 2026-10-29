// Byte and text helpers shared by the relay. Everything here runs unchanged in
// a Cloudflare Worker and in Bun's test runner: only Web APIs are used.

const encoder = new TextEncoder();
const strictDecoder = new TextDecoder("utf-8", { fatal: true });

export function utf8(text: string): Uint8Array {
  return encoder.encode(text);
}

export function fromUtf8(bytes: Uint8Array): string | null {
  try {
    return strictDecoder.decode(bytes);
  } catch {
    return null;
  }
}

export function concatBytes(...parts: Uint8Array[]): Uint8Array {
  const total = parts.reduce((sum, part) => sum + part.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

export function base64UrlEncode(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/u, "");
}

/** Decodes unpadded (or padded) base64url. Returns null for anything else. */
export function base64UrlDecode(text: string): Uint8Array | null {
  const unpadded = text.replace(/=+$/u, "");
  if (!/^[A-Za-z0-9_-]*$/u.test(unpadded) || unpadded.length % 4 === 1) {
    return null;
  }
  const padded =
    unpadded.replace(/-/g, "+").replace(/_/g, "/") +
    "===".slice((unpadded.length + 3) % 4);
  let binary: string;
  try {
    binary = atob(padded);
  } catch {
    return null;
  }
  const out = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    out[index] = binary.charCodeAt(index);
  }
  return out;
}

export function base64Decode(text: string): Uint8Array | null {
  try {
    const binary = atob(text.replace(/\s+/gu, ""));
    const out = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index += 1) {
      out[index] = binary.charCodeAt(index);
    }
    return out;
  } catch {
    return null;
  }
}

/** A random identifier of `bytes` bytes of entropy, base64url encoded. */
export function randomId(bytes: number): string {
  const buffer = new Uint8Array(bytes);
  crypto.getRandomValues(buffer);
  return base64UrlEncode(buffer);
}

export async function sha256Hex(text: string): Promise<string> {
  const digest = new Uint8Array(
    await crypto.subtle.digest("SHA-256", toArrayBuffer(utf8(text))),
  );
  return Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

/** Compares two byte strings without an early exit on the first difference. */
export function constantTimeEqual(left: Uint8Array, right: Uint8Array): boolean {
  let difference = left.length ^ right.length;
  const length = Math.max(left.length, right.length);
  for (let index = 0; index < length; index += 1) {
    difference |= (left[index] ?? 0) ^ (right[index] ?? 0);
  }
  return difference === 0;
}

/**
 * WebCrypto wants a BufferSource backed by an ArrayBuffer. A Uint8Array that
 * views part of a larger (or shared) buffer is copied into its own buffer.
 */
export function toArrayBuffer(bytes: Uint8Array): ArrayBuffer {
  if (
    bytes.byteOffset === 0 &&
    bytes.byteLength === bytes.buffer.byteLength &&
    bytes.buffer instanceof ArrayBuffer
  ) {
    return bytes.buffer;
  }
  return bytes.slice().buffer as ArrayBuffer;
}

/** Reads a PEM block (for example `PRIVATE KEY`) into DER bytes. */
export function pemToDer(pem: string, label: string): Uint8Array | null {
  const pattern = new RegExp(
    `-----BEGIN ${label}-----([A-Za-z0-9+/=\\s]+)-----END ${label}-----`,
    "u",
  );
  const match = pem.replace(/\\n/gu, "\n").match(pattern);
  if (!match?.[1]) return null;
  return base64Decode(match[1]);
}
