import { expect, test } from "bun:test";
import { base64UrlDecode, base64UrlEncode, utf8 } from "../src/encoding.ts";
import {
  decryptWebPush,
  encryptWebPush,
  generateUserAgentKeys,
  importP256Private,
} from "../tools/web-push.ts";

const b64 = (text: string) => {
  const bytes = base64UrlDecode(text);
  if (!bytes) throw new Error(`bad vector value ${text}`);
  return bytes;
};

// RFC 8291, Appendix A.
const VECTOR = {
  plaintext: "V2hlbiBJIGdyb3cgdXAsIEkgd2FudCB0byBiZSBhIHdhdGVybWVsb24",
  asPublic: "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8",
  asPrivate: "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw",
  uaPublic: "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
  uaPrivate: "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94",
  salt: "DGv6ra1nlYgDCS1FRnbzlw",
  authSecret: "BTBZMqHH6r4Tts7J_aSIgg",
  body:
    "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN",
};

test("encryption reproduces the RFC 8291 Appendix A message", async () => {
  const asPublic = b64(VECTOR.asPublic);
  const body = await encryptWebPush({
    uaPublic: b64(VECTOR.uaPublic),
    authSecret: b64(VECTOR.authSecret),
    plaintext: b64(VECTOR.plaintext),
    salt: b64(VECTOR.salt),
    senderKeys: { publicKey: asPublic, privateKey: await importP256Private(b64(VECTOR.asPrivate), asPublic, "ECDH") },
  });
  expect(base64UrlEncode(body)).toBe(VECTOR.body);
});

test("decryption recovers the RFC 8291 Appendix A plaintext", async () => {
  const uaPublic = b64(VECTOR.uaPublic);
  const plaintext = await decryptWebPush({
    uaPrivate: await importP256Private(b64(VECTOR.uaPrivate), uaPublic, "ECDH"),
    uaPublic,
    authSecret: b64(VECTOR.authSecret),
    body: b64(VECTOR.body),
  });
  expect(new TextDecoder().decode(plaintext)).toBe("When I grow up, I want to be a watermelon");
});

test("a fresh subscription round-trips a Declarative Web Push payload", async () => {
  const ua = await generateUserAgentKeys();
  const payload = JSON.stringify({ web_push: 8030, notification: { title: "Yurucommu", navigate: "https://y.example/" } });
  const body = await encryptWebPush({ uaPublic: ua.publicKey, authSecret: ua.authSecret, plaintext: utf8(payload) });
  const plaintext = await decryptWebPush({ uaPrivate: ua.privateKey, uaPublic: ua.publicKey, authSecret: ua.authSecret, body });
  expect(new TextDecoder().decode(plaintext)).toBe(payload);
});
