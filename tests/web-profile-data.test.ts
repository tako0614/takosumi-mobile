import { expect, test } from "bun:test";
import { webConnection } from "../src/web-connections.ts";
import { clearWebProfileData, webProfileClearRequest } from "../src/web-profile-data.ts";

const profile = "11111111-1111-4111-8111-111111111111";
const connection = webConnection("https://work.example", "Work", profile);

test("clear data targets the isolated connection, without changing its saved identity", async () => {
  const calls: unknown[] = [];
  await clearWebProfileData(connection, async <T>(command: string, args?: Record<string, unknown>) => {
    calls.push([command, args]);
    return { cleared: true } as T;
  });
  expect(calls).toEqual([["plugin:app-screen|clear_profile", {
    payload: { profile, key: `web-home:${profile}:https://work.example` },
  }]]);
  expect(connection.profile).toBe(profile);
});

test("the legacy shared profile and malformed targets never reach native deletion", async () => {
  let calls = 0;
  for (const target of [
    webConnection("https://work.example", "Work", "shared"),
    { ...connection, profile: "../shared" },
    { ...connection, origin: "https://work.example/?token=secret" },
  ]) {
    await expect(clearWebProfileData(target, async <T>() => {
      calls++;
      return { cleared: true } as T;
    })).rejects.toThrow();
  }
  expect(calls).toBe(0);
  expect(webProfileClearRequest(connection)).toEqual({ profile, key: `web-home:${profile}:https://work.example` });
});

test("clear waits for an explicit completion receipt and propagates failure", async () => {
  for (const value of [undefined, null, {}, { cleared: false }, { cleared: "true" }]) {
    await expect(clearWebProfileData(connection, async <T>() => value as T)).rejects.toThrow("消去を確認");
  }
  await expect(clearWebProfileData(connection, async () => {
    throw new Error("Close this connection's screens first.");
  })).rejects.toThrow("Close this connection");
  await expect(clearWebProfileData(connection, async () => {
    throw "この接続先の画面をすべて閉じてから消去してください。";
  })).rejects.toThrow("すべて閉じて");
  let finish!: (value: unknown) => void;
  let completed = false;
  const result = clearWebProfileData(connection, <T>() => new Promise<unknown>((resolve) => { finish = resolve; }) as Promise<T>)
    .then(() => { completed = true; });
  await Promise.resolve();
  expect(completed).toBe(false);
  finish({ cleared: true });
  await result;
  expect(completed).toBe(true);
});
