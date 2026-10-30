import { expect, test } from "bun:test";
import {
  accountForTap,
  createPendingNotification,
  routeNotification,
  tapIsForSession,
} from "../src/push-routing.ts";

const origins = ["https://accounts.takosumi.example", "https://takosumi.example"];

test("a Run page opens the Run screen; other pages of the instance open in the browser view", () => {
  expect(routeNotification("https://takosumi.example/runs/run_123", origins)).toEqual({
    kind: "run",
    runId: "run_123",
    url: "https://takosumi.example/runs/run_123",
  });
  expect(routeNotification("https://takosumi.example/runs/run%3A9/", origins)).toMatchObject({
    kind: "run",
    runId: "run:9",
  });
  expect(routeNotification("https://accounts.takosumi.example/runs/r1", origins)).toMatchObject({ kind: "run" });
  for (const page of [
    "https://takosumi.example/apps/cap_1",
    "https://takosumi.example/apps/cap_1#attention",
    "https://takosumi.example/apps/cap_1#links",
    "https://takosumi.example/connections",
    "https://takosumi.example/runs/r1/logs",
    "https://takosumi.example/runs/",
    "https://takosumi.example/",
  ]) {
    expect(routeNotification(page, origins)).toEqual({ kind: "page", url: new URL(page).href });
  }
});

test("anything off the instance's origins, or not a page, opens the home", () => {
  for (const navigate of [
    undefined,
    "",
    "not a url",
    "/runs/run_1",
    "https://evil.example/runs/run_1",
    "https://takosumi.example.evil/runs/run_1",
    "https://user:pass@takosumi.example/runs/run_1",
    "http://takosumi.example/runs/run_1",
    "javascript:alert(1)",
    "takosumi://app?url=https://takosumi.example/",
    "https://takosumi.example:8443/runs/run_1",
  ]) {
    expect(routeNotification(navigate, origins)).toEqual({ kind: "home" });
  }
  // A Run id that is not one is still the instance's page, not a Run.
  expect(routeNotification("https://takosumi.example/runs/%E0%A4%A", origins)).toMatchObject({ kind: "page" });
  expect(routeNotification("https://takosumi.example/runs/a%2Fb", origins)).toMatchObject({ kind: "page" });
});

test("a tap belongs to the saved account with the same instance origin and subject", () => {
  const accounts = [
    { id: "a", hostUrl: "https://takosumi.example", accountId: "alice" },
    { id: "b", hostUrl: "https://takosumi.example/", accountId: "bob" },
    { id: "c", hostUrl: "https://other.example", accountId: "alice" },
    { id: "d", hostUrl: "https://legacy.example" },
  ];
  expect(accountForTap({ instance: "https://takosumi.example", account: "bob" }, accounts)?.id).toBe("b");
  expect(accountForTap({ instance: "https://other.example", account: "alice" }, accounts)?.id).toBe("c");
  expect(accountForTap({ instance: "https://legacy.example", account: "" }, accounts)?.id).toBe("d");
  expect(accountForTap({ instance: "https://takosumi.example", account: "carol" }, accounts)).toBeUndefined();
  expect(accountForTap({ instance: "not an origin", account: "alice" }, accounts)).toBeUndefined();
  expect(tapIsForSession({ instance: "https://takosumi.example", account: "alice" }, accounts[0]!)).toBe(true);
  expect(tapIsForSession({ instance: "https://takosumi.example", account: "alice" }, accounts[1]!)).toBe(false);
});

test("a pending notification is taken once, and only by its own session", () => {
  const pending = createPendingNotification();
  let notified = 0;
  const unsubscribe = pending.subscribe(() => {
    notified += 1;
  });
  pending.set({ identity: "https://takosumi.example\u0000alice", navigate: "https://takosumi.example/runs/1" });
  expect(notified).toBe(1);
  expect(pending.takeFor({ hostUrl: "https://takosumi.example", accountId: "bob" })).toBeUndefined();
  expect(pending.takeFor({ hostUrl: "https://takosumi.example", accountId: "alice" })?.navigate).toBe(
    "https://takosumi.example/runs/1",
  );
  expect(pending.takeFor({ hostUrl: "https://takosumi.example", accountId: "alice" })).toBeUndefined();
  unsubscribe();
  pending.set({ identity: "x" });
  expect(notified).toBe(1);
});
