import { expect, test } from "bun:test";
import { base64UrlDecode } from "../relay/src/encoding.ts";
import { decryptWebPush, importP256Private } from "../relay/tools/web-push.ts";
import { routeNotification } from "../src/push-routing.ts";

/**
 * `src-tauri/plugins/push/web-push-cases.json` is the table the Android
 * plugin's WebPushCasesTest decrypts and parses with the device's own code.
 * Here the relay's standards Web Push tool checks the same messages, and the
 * shell's tap routing agrees with the device on which pages may open.
 */

interface Case {
  readonly name: string;
  readonly body: string;
  readonly authSecret?: string;
  readonly decrypts: boolean;
  readonly plaintext?: string;
  readonly notification: { readonly navigate?: string } | null;
}

const table = (await Bun.file(new URL("../src-tauri/plugins/push/web-push-cases.json", import.meta.url)).json()) as {
  readonly rfc8291: Record<string, string>;
  readonly subscription: {
    readonly uaPublic: string;
    readonly uaPrivate: string;
    readonly authSecret: string;
    readonly origins: readonly string[];
  };
  readonly cases: readonly Case[];
};

const bytes = (text: string) => {
  const decoded = base64UrlDecode(text);
  if (!decoded) throw new Error(`not base64url: ${text}`);
  return decoded;
};

test("the table holds the RFC 8291 Appendix A message", async () => {
  const rfc = table.rfc8291;
  const uaPublic = bytes(rfc.uaPublic!);
  const plaintext = await decryptWebPush({
    uaPrivate: await importP256Private(bytes(rfc.uaPrivate!), uaPublic, "ECDH"),
    uaPublic,
    authSecret: bytes(rfc.authSecret!),
    body: bytes(rfc.body!),
  });
  expect(new TextDecoder().decode(plaintext)).toBe("When I grow up, I want to be a watermelon");
});

test("every case decrypts (or not) as the table says, and taps open only allowed pages", async () => {
  const { subscription } = table;
  const uaPublic = bytes(subscription.uaPublic);
  const uaPrivate = await importP256Private(bytes(subscription.uaPrivate), uaPublic, "ECDH");
  expect(table.cases.length).toBeGreaterThanOrEqual(10);
  for (const item of table.cases) {
    let plaintext: string | undefined;
    try {
      plaintext = new TextDecoder().decode(
        await decryptWebPush({
          uaPrivate,
          uaPublic,
          authSecret: bytes(item.authSecret ?? subscription.authSecret),
          body: bytes(item.body),
        }),
      );
    } catch {
      plaintext = undefined;
    }
    expect({ name: item.name, decrypts: plaintext !== undefined }).toEqual({ name: item.name, decrypts: item.decrypts });
    if (plaintext === undefined) continue;
    expect(plaintext).toBe(item.plaintext!);
    if (!item.notification) continue;
    // What the device keeps as `navigate` is what the shell will open.
    const sent = (JSON.parse(plaintext) as { notification: { navigate?: string } }).notification.navigate;
    const route = routeNotification(sent, subscription.origins);
    expect({ name: item.name, opens: route.kind !== "home" }).toEqual({
      name: item.name,
      opens: item.notification.navigate !== undefined,
    });
  }
});
