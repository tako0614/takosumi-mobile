import { expect, test } from "bun:test";
import {
  clientForPlatform,
  clientLabel,
  clientStoreHost,
  clientTargetLabel,
  decodeScreenClients,
  openClientRequest,
  UNVERIFIED_CLIENT_LABEL,
} from "../src/app-clients.ts";
import { appScreenOpenRequest, CAPSULE_APP_SCREENS_DECODER } from "../src/app-screens.ts";

// The shape `GET /api/v1/capsules/{id}/app` returns (takosumi #689).
const verified = {
  preferred: "native",
  ios: {
    verified: true,
    trustedClientId: "yurucommu",
    target: { scheme: "yurucommu" },
    name: "Yurucommu",
    appStoreId: "1234567890",
    bundleId: "com.yurucommu",
    open: "yurucommu://connect?host_url=https%3A%2F%2Fyc.example&product=yurucommu",
    storeUrl: "https://apps.apple.com/app/id1234567890",
  },
  android: {
    verified: true,
    trustedClientId: "yurucommu",
    target: { package: "com.yurucommu", scheme: "yurucommu" },
    name: "Yurucommu",
    package: "com.yurucommu",
    open: "yurucommu://connect?host_url=https%3A%2F%2Fyc.example&product=yurucommu",
    storeUrl: "https://play.google.com/store/apps/details?id=com.yurucommu",
  },
};

test("verified clients keep their trusted name, links and store page", () => {
  const clients = decodeScreenClients(verified);
  expect(clients?.preferred).toBe("native");
  expect(clients?.android).toEqual({
    platform: "android",
    verified: true,
    name: "Yurucommu",
    package: "com.yurucommu",
    target: { package: "com.yurucommu", scheme: "yurucommu" },
    open: "yurucommu://connect?host_url=https%3A%2F%2Fyc.example&product=yurucommu",
    storeUrl: "https://play.google.com/store/apps/details?id=com.yurucommu",
  });
  expect(clients?.ios?.verified).toBe(true);
  expect(clientLabel(clients!.ios!)).toBe("Yurucommu");
  expect(openClientRequest(clients!.android!)).toEqual({
    platform: "android",
    package: "com.yurucommu",
    open: "yurucommu://connect?host_url=https%3A%2F%2Fyc.example&product=yurucommu",
  });
  expect(openClientRequest(clients!.ios!)).toEqual({
    platform: "ios",
    open: "yurucommu://connect?host_url=https%3A%2F%2Fyc.example&product=yurucommu",
  });
  expect(clientForPlatform(clients, "android")).toBe(clients!.android);
  expect(clientForPlatform(clients, "web")).toBeUndefined();
});

test("unverified clients show a neutral label and the real destination, never the title", () => {
  const clients = decodeScreenClients({
    preferred: "web",
    android: {
      verified: false,
      // The instance's target is ignored; the links themselves are shown.
      target: { package: "com.innocent.example" },
      package: "com.example.chat",
      appLink: "https://open.chat.example/connect#host_url=https%3A%2F%2Fx.example",
      storeUrl: "https://f-droid.org/packages/com.example.chat/",
    },
    ios: {
      verified: false,
      name: "Chat",
      universalLink: "https://open.chat.example/connect",
      open: "chatapp://connect",
    },
  });
  expect(clients?.preferred).toBe("web");
  const android = clients!.android!;
  expect(android.verified).toBe(false);
  expect(clientLabel(android)).toBe(UNVERIFIED_CLIENT_LABEL);
  expect(clientTargetLabel(android)).toBe("com.example.chat · open.chat.example");
  expect(clientStoreHost(android)).toBe("f-droid.org");
  expect(clientLabel(clients!.ios!)).toBe("Chat");
  expect(clientTargetLabel(clients!.ios!)).toBe("open.chat.example · chatapp://");
  expect(clientStoreHost(clients!.ios!)).toBeUndefined();
});

test("a verified flag without the trusted name, or preferred native without a verified client, is not trusted", () => {
  const clients = decodeScreenClients({
    preferred: "native",
    android: { verified: true, package: "com.example.chat", open: "chatapp://x" },
  });
  expect(clients?.android?.verified).toBe(false);
  expect(clients?.preferred).toBe("web");
});

test("links the shell must not open are dropped, and a platform without a link with them", () => {
  const decode = (android: Record<string, unknown>) =>
    decodeScreenClients({ android: { verified: false, package: "com.example.chat", ...android } })
      ?.android;
  for (const open of [
    "takosumi://oauth/callback?code=x",
    "intent://x#Intent;scheme=chatapp;end",
    "javascript:alert(1)",
    "https://chat.example/",
    "tel:+810000",
    "itms-services://?action=download-manifest",
    "googlechrome://evil.example",
    "Chatapp://x",
    "chat app://x",
    "chatapp://x\n",
  ]) {
    expect({ open, decoded: decode({ open }) }).toEqual({ open, decoded: undefined });
  }
  expect(decode({ appLink: "http://open.chat.example/" })).toBeUndefined();
  expect(decode({ appLink: "https://u:p@open.chat.example/" })).toBeUndefined();
  // Takosumi Mobile itself can never be the target.
  expect(
    decodeScreenClients({ android: { verified: false, package: "com.takosumi", open: "chatapp://x" } }),
  ).toBeUndefined();
  expect(decode({ package: "not a package", open: "chatapp://x" })).toBeUndefined();
  // A store page elsewhere falls back to the package's Google Play page.
  expect(decode({ open: "chatapp://x", storeUrl: "https://evil.example/app" })?.storeUrl).toBe(
    "https://play.google.com/store/apps/details?id=com.example.chat",
  );
  // A name with bidi overrides or line breaks is dropped, not shown.
  expect(decode({ open: "chatapp://x", name: "Chat\u202Egnp.exe" })?.name).toBeUndefined();
  expect(decode({ open: "chatapp://x", name: "Chat\nApp" })?.name).toBeUndefined();
});

test("screens carry their clients into the launcher and the banner gets only what it needs", () => {
  const view = CAPSULE_APP_SCREENS_DECODER.decode({
    app: {
      kind: "takosumi.capsule-app@v1",
      capsuleId: "cap_a",
      screens: [
        { interfaceId: "web", name: "web", title: "Yurucommu", url: "https://yc.example/", clients: verified },
        {
          interfaceId: "admin",
          name: "admin",
          url: "https://yc.example/admin",
          clients: { android: { verified: false, package: "com.example.chat", open: "chatapp://x" } },
        },
        { interfaceId: "plain", name: "plain", url: "https://yc.example/p", clients: "nonsense" },
      ],
    },
  });
  expect(view.screens[0]?.clients?.preferred).toBe("native");
  expect(view.screens[2]?.clients).toBeUndefined();

  const target = { key: "app:cap_a:web", label: "Yurucommu", url: "https://yc.example/" };
  expect(appScreenOpenRequest({ ...target, clients: view.screens[0]!.clients! }, undefined, {
    platform: "android",
    accountsOrigin: "https://accounts.example",
    scope: "https://takosumi.example|sub_1",
    resume: true,
  })).toEqual({
    key: "app:cap_a:web",
    url: "https://yc.example/",
    title: "Yurucommu",
    profile: "shared",
    accountsOrigin: "https://accounts.example",
    scope: "https://takosumi.example|sub_1",
    resume: true,
    client: {
      name: "Yurucommu",
      verified: true,
      package: "com.yurucommu",
      open: "yurucommu://connect?host_url=https%3A%2F%2Fyc.example&product=yurucommu",
    },
  });
  // An unverified client's links never reach the bar: it asks in the chooser.
  expect(
    appScreenOpenRequest({ ...target, clients: view.screens[1]!.clients! }, undefined, {
      platform: "android",
    }).client,
  ).toEqual({ name: UNVERIFIED_CLIENT_LABEL, verified: false });
  expect(
    appScreenOpenRequest({ ...target, clients: view.screens[0]!.clients! }, undefined, {
      platform: "web",
    }).client,
  ).toBeUndefined();
});
