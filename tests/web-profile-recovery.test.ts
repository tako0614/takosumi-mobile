import { expect, test } from "bun:test";
import { webConnection, webHomeRequest, connectionKey } from "../src/web-connections.ts";
import { BrowserRecoveryRequired, openWebConnection, recoverWebProfile } from "../src/web-profile-recovery.ts";

const connection = webConnection("https://work.example", "Work", "11111111-1111-4111-8111-111111111111");
const nativeError = {
  kind: "browserProfileRecoveryRequired", profile: connection.profile,
  key: connectionKey(connection), generation: "17",
};

test("opening reports a native crash ticket but never starts recovery implicitly", async () => {
  const calls: string[] = [];
  let caught: unknown;
  try {
    await openWebConnection(connection, async (command) => { calls.push(command); throw nativeError; });
  } catch (cause) { caught = cause; }
  expect(caught).toBeInstanceOf(BrowserRecoveryRequired);
  expect(calls).toEqual(["plugin:app-screen|open"]);
  expect((caught as BrowserRecoveryRequired).generation).toBe("17");
});

test("a string or another profile, key, or malformed generation cannot authorize recovery", async () => {
  for (const cause of [
    "Previous browser process ended abnormally; profile data is retained.",
    { ...nativeError, kind: "different" },
    { ...nativeError, profile: "22222222-2222-4222-8222-222222222222" },
    { ...nativeError, key: "web-home:other:https://work.example" },
    ...["", "0", "01", "-1", "1e3", "../17", "1".repeat(21), 17].map((generation) => ({ ...nativeError, generation })),
  ]) {
    let caught: unknown;
    try { await openWebConnection(connection, async () => { throw cause; }); }
    catch (error) { caught = error; }
    expect(caught).toBeInstanceOf(Error);
    expect(caught).not.toBeInstanceOf(BrowserRecoveryRequired);
  }
});

test("confirmed recovery retains the original saved identity and expected native generation", async () => {
  const calls: unknown[] = [];
  const ticket = new BrowserRecoveryRequired(connection.profile, connectionKey(connection), "17");
  await recoverWebProfile(connection, ticket, async <T>(command: string, args?: Record<string, unknown>) => {
    calls.push([command, args]);
    return { reopened: true } as T;
  });
  expect(calls).toEqual([["plugin:app-screen|recover_profile", {
    payload: { open: webHomeRequest(connection), generation: "17" },
  }]]);
  expect(connection.profile).toBe(nativeError.profile);
});

test("shared, changed or malformed connections and tickets never reach native recovery", async () => {
  const ticket = new BrowserRecoveryRequired(connection.profile, connectionKey(connection), "17");
  let calls = 0;
  for (const target of [
    webConnection("https://work.example", "Work", "shared"),
    webConnection("https://personal.example", "Personal", "22222222-2222-4222-8222-222222222222"),
    { ...connection, origin: "https://work.example/?token=secret" },
  ]) {
    await expect(recoverWebProfile(target, ticket, async <T>() => { calls++; return { reopened: true } as T; })).rejects.toThrow();
  }
  await expect(recoverWebProfile(connection, { ...ticket } as BrowserRecoveryRequired,
    async <T>() => { calls++; return { reopened: true } as T; })).rejects.toThrow();
  expect(calls).toBe(0);
});

test("recovery waits for native reopen and rejects absent or negative completion", async () => {
  const ticket = new BrowserRecoveryRequired(connection.profile, connectionKey(connection), "17");
  for (const value of [undefined, null, {}, { reopened: false }, { reopened: "true" }]) {
    await expect(recoverWebProfile(connection, ticket, async <T>() => value as T)).rejects.toThrow("復帰を確認");
  }
  await expect(recoverWebProfile(connection, ticket, async () => { throw "stale recovery"; })).rejects.toThrow("stale recovery");
  let finish!: (value: unknown) => void;
  let completed = false;
  const result = recoverWebProfile(connection, ticket, <T>() => new Promise<unknown>((resolve) => { finish = resolve; }) as Promise<T>)
    .then(() => { completed = true; });
  await Promise.resolve();
  expect(completed).toBe(false);
  finish({ reopened: true });
  await result;
  expect(completed).toBe(true);
});
