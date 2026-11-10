import { expect, test } from "bun:test";
import {
  createWebPermissionPrompts,
  decodePendingPermissions,
  permissionLabels,
  type PendingPermission,
} from "../src/web-permissions.ts";

const profile = "11111111-1111-4111-8111-111111111111";
const prompt: PendingPermission = {
  profile,
  promptId: 7,
  origin: "https://work.example",
  kind: "prompt",
  permissions: 1 << 15,
};

test("answer sends the held prompt's pair and returns the prompts still held", async () => {
  const calls: unknown[] = [];
  const remaining: PendingPermission[] = [{
    profile,
    promptId: 8,
    origin: "https://work.example",
    kind: "media",
    permissions: 3,
  }];
  const list = await createWebPermissionPrompts(
    async <T>(command: string, args?: Record<string, unknown>) => {
      calls.push([command, args]);
      return remaining as T;
    },
  ).answer(prompt, "allow");
  expect(calls).toEqual([["plugin:app-screen|resolve_permission", {
    payload: { profile, promptId: 7, decision: "allow" },
  }]]);
  expect(list).toEqual(remaining);
});

test("list decodes only well-formed held prompts and tolerates transport failure", async () => {
  const good = {
    profile,
    promptId: 7,
    origin: "https://work.example",
    kind: "prompt",
    permissions: 8,
  };
  const decoded = decodePendingPermissions([
    good,
    null,
    { promptId: "7" },
    { ...good, promptId: 0 },
    // Any origin text passes through: filtering one would hold its browser
    // callback forever with no way to answer it.
    { ...good, origin: "http://insecure.example" },
    { ...good, origin: 7 },
    { ...good, permissions: -1 },
  ]);
  expect(decoded).toEqual([
    {
      profile,
      promptId: 7,
      origin: "https://work.example",
      kind: "prompt",
      permissions: 8,
    },
    {
      profile,
      promptId: 7,
      origin: "http://insecure.example",
      kind: "prompt",
      permissions: 8,
    },
  ]);
  expect(decodePendingPermissions({})).toBeUndefined();
  const list = await createWebPermissionPrompts(async <T>() => {
    throw new Error("down") as T;
  }).list();
  expect(list).toEqual([]);
});

test("answer propagates a transport error so the held prompt stays", async () => {
  let caught: unknown;
  try {
    await createWebPermissionPrompts(async <T>() => {
      throw "refused" as T;
    }).answer(prompt, "deny");
  } catch (cause) {
    caught = cause;
  }
  expect(caught).toBe("refused");
});

test("labels name the CEF permission bits a site actually asks for", () => {
  expect(permissionLabels(1 << 15)).toEqual(["通知"]);
  expect(permissionLabels((1 << 2) | (1 << 12))).toEqual(["カメラ", "マイク"]);
  expect(permissionLabels(0)).toEqual(["このサイトの機能"]);
  expect(permissionLabels(1 << 30)).toEqual(["このサイトの機能"]);
});
