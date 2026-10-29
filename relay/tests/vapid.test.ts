import { expect, test } from "bun:test";
import { base64UrlEncode, utf8 } from "../src/encoding.ts";
import { parseVapidAuthorization, verifyVapid } from "../src/vapid.ts";
import { generateVapidKeys, vapidAuthorization } from "../tools/web-push.ts";

const AUD = "https://push.relay.test";
const NOW = 1_800_000_000;

async function setup() {
  const keys = await generateVapidKeys();
  const header = await vapidAuthorization(keys, AUD, { subject: "mailto:a@example.com", expiresAt: NOW + 3600 });
  return { keys, header };
}

function expected(keys: { publicKey: Uint8Array }, overrides: Partial<{ audience: string; now: number }> = {}) {
  return { audience: AUD, applicationServerKey: keys.publicKey, now: NOW, ...overrides };
}

test("parses the vapid scheme with t and k in any order and case", () => {
  expect(parseVapidAuthorization("vapid t=a.b.c, k=KEY")).toEqual({ token: "a.b.c", key: "KEY" });
  expect(parseVapidAuthorization('VAPID k="KEY",t=a.b.c')).toEqual({ token: "a.b.c", key: "KEY" });
  expect(parseVapidAuthorization("WebPush a.b.c")).toBeNull();
  expect(parseVapidAuthorization("vapid t=a.b.c")).toBeNull();
  expect(parseVapidAuthorization("vapid t=a, t=b, k=c")).toBeNull();
  expect(parseVapidAuthorization(null)).toBeNull();
});

test("accepts a JWT signed by the subscription's key for this origin", async () => {
  const { keys, header } = await setup();
  expect(await verifyVapid(header, expected(keys))).toEqual({ ok: true });
  // aud with a trailing slash names the same origin.
  const slash = await vapidAuthorization(keys, `${AUD}/`, { expiresAt: NOW + 60 });
  expect(await verifyVapid(slash, expected(keys))).toEqual({ ok: true });
});

test("answers 401 when the credential is absent or malformed", async () => {
  const { keys } = await setup();
  const k = base64UrlEncode(keys.publicKey);
  for (const header of [null, "Bearer x", `vapid t=not-a-jwt, k=${k}`, "vapid t=a.b.c, k=!!!", `vapid t=a.b.c, k=${base64UrlEncode(new Uint8Array(65))}`]) {
    const result = await verifyVapid(header, expected(keys));
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.failure.status).toBe(401);
  }
  const none = await vapidAuthorization(keys, AUD, { expiresAt: NOW + 60, header: { typ: "JWT", alg: "none" } });
  const result = await verifyVapid(none, expected(keys));
  expect(!result.ok && result.failure).toMatchObject({ status: 401, code: "vapid_malformed" });
});

test("answers 403 when k is not the subscription's application server key", async () => {
  const { keys } = await setup();
  const other = await generateVapidKeys();
  const header = await vapidAuthorization(other, AUD, { expiresAt: NOW + 60 });
  const result = await verifyVapid(header, expected(keys));
  expect(!result.ok && result.failure).toMatchObject({ status: 403, code: "vapid_key_mismatch" });
});

test("answers 403 for a forged signature", async () => {
  const { keys, header } = await setup();
  const forger = await generateVapidKeys();
  // The forger signs with its own key but claims the subscription's key.
  const forged = await vapidAuthorization(forger, AUD, { expiresAt: NOW + 60 });
  const token = forged.match(/t=([^,]+)/u)?.[1];
  const result = await verifyVapid(`vapid t=${token}, k=${base64UrlEncode(keys.publicKey)}`, expected(keys));
  expect(!result.ok && result.failure).toMatchObject({ status: 403, code: "vapid_signature_invalid" });
  // A modified claim invalidates the original signature too.
  const [h, , s] = (header.match(/t=([^,]+)/u)?.[1] ?? "").split(".");
  const claims = base64UrlEncode(utf8(JSON.stringify({ aud: AUD, exp: NOW + 7200 })));
  const tampered = await verifyVapid(`vapid t=${h}.${claims}.${s}, k=${base64UrlEncode(keys.publicKey)}`, expected(keys));
  expect(!tampered.ok && tampered.failure.code).toBe("vapid_signature_invalid");
});

test("answers 403 when aud is another origin", async () => {
  const { keys } = await setup();
  for (const aud of ["https://other.example", "https://push.relay.test/push", "http://push.relay.test"]) {
    const header = await vapidAuthorization(keys, aud, { expiresAt: NOW + 60 });
    const result = await verifyVapid(header, expected(keys));
    expect(!result.ok && result.failure).toMatchObject({ status: 403, code: "vapid_audience_mismatch" });
  }
});

test("answers 403 for an expired JWT or one that lives longer than 24 hours", async () => {
  const { keys } = await setup();
  const expired = await vapidAuthorization(keys, AUD, { expiresAt: NOW - 120 });
  const tooLong = await vapidAuthorization(keys, AUD, { expiresAt: NOW + 24 * 3600 + 3600 });
  const atLimit = await vapidAuthorization(keys, AUD, { expiresAt: NOW + 24 * 3600 });
  const expiredResult = await verifyVapid(expired, expected(keys));
  const tooLongResult = await verifyVapid(tooLong, expected(keys));
  expect(!expiredResult.ok && expiredResult.failure).toMatchObject({ status: 403, code: "vapid_expired" });
  expect(!tooLongResult.ok && tooLongResult.failure).toMatchObject({ status: 403, code: "vapid_expiry_too_far" });
  expect(await verifyVapid(atLimit, expected(keys))).toEqual({ ok: true });
});
