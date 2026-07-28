import { expect, test } from "bun:test";
import type {
  MobileKeyValueStore,
  MobileSession,
} from "@takosjp/mobile-kit";
import {
  createWorkspacePreference,
  workspacePreferenceKey,
} from "../src/workspace-preference.ts";

const session = {
  hostUrl: "https://operator.example",
  product: "takosumi",
  oidcClientId: "takosumi-mobile-operator",
  accessToken: "access",
  tokenType: "Bearer",
  createdAt: "2026-07-20T00:00:00.000Z",
} satisfies MobileSession;

test("the selected Workspace is persisted per host and client", async () => {
  const values = new Map<string, string>();
  const store: MobileKeyValueStore = {
    kind: "device-persistent",
    get: async (key) => values.get(key),
    set: async (key, value) => {
      values.set(key, value);
    },
    delete: async (key) => {
      values.delete(key);
    },
  };
  const preference = createWorkspacePreference(store);

  await preference.save(session, "ws_team");
  expect(await preference.load(session)).toBe("ws_team");
  expect([...values.keys()]).toEqual([workspacePreferenceKey(session)]);

  const otherHost = { ...session, hostUrl: "https://other.example" };
  expect(await preference.load(otherHost)).toBeUndefined();
});

test("Workspace preference degrades to memory-only when storage is unavailable", async () => {
  const preference = createWorkspacePreference(undefined);
  await preference.save(session, "ws_personal");
  expect(await preference.load(session)).toBe("ws_personal");
});
