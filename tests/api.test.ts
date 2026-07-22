import { afterEach, expect, test } from "bun:test";
import type { MobileSession } from "@takosjp/mobile-kit";
import {
  approveRun,
  cancelRun,
  loadHome,
  loadWorkspaceHome,
} from "../src/api.ts";

const originalFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = originalFetch;
});

const session: MobileSession = {
  hostUrl: "https://operator.example",
  product: "takosumi",
  accessToken: "mobile-access",
  tokenType: "Bearer",
  createdAt: "2026-07-20T00:00:00.000Z",
};

test("home uses bearer auth and loads the first Workspace", async () => {
  const paths: string[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input));
    paths.push(url.pathname + url.search);
    expect(new Headers(init?.headers).get("authorization")).toBe(
      "Bearer mobile-access",
    );
    if (url.pathname === "/api/v1/workspaces") {
      return Response.json({
        workspaces: [
          { id: "ws_a", handle: "a", displayName: "A", type: "personal" },
        ],
      });
    }
    if (url.pathname.endsWith("/capsules")) {
      return Response.json({ capsules: [{ id: "cap_a", name: "Takos" }] });
    }
    return Response.json({ runs: [{ id: "run_a", status: "running" }] });
  }) as typeof fetch;

  const home = await loadHome(session);
  expect(home.workspace?.id).toBe("ws_a");
  expect(home.capsules).toHaveLength(1);
  expect(home.runs).toHaveLength(1);
  expect(paths.sort()).toEqual([
    "/api/v1/workspaces",
    "/api/v1/workspaces/ws_a/capsules?includeDestroyed=false",
    "/api/v1/workspaces/ws_a/runs?limit=30",
  ]);
});

test("Workspace switching reuses the known Workspace list", async () => {
  let workspaceListRequests = 0;
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    if (url.pathname === "/api/v1/workspaces") workspaceListRequests += 1;
    if (url.pathname.endsWith("/capsules"))
      return Response.json({ capsules: [] });
    return Response.json({ runs: [] });
  }) as typeof fetch;
  const workspaces = [
    { id: "ws_b", handle: "b", displayName: "B", type: "team" as const },
  ];
  const home = await loadWorkspaceHome(session, "ws_b", workspaces);
  expect(home.workspace?.displayName).toBe("B");
  expect(workspaceListRequests).toBe(0);
});

test("approval and cancellation stay on bearer-authenticated Run routes", async () => {
  const calls: Array<{ path: string; method?: string; body?: unknown }> = [];
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input));
    calls.push({
      path: url.pathname,
      method: init?.method,
      body: init?.body ? await new Response(init.body).json() : undefined,
    });
    expect(new Headers(init?.headers).get("authorization")).toBe(
      "Bearer mobile-access",
    );
    return Response.json({ run: { id: "run_a", status: "succeeded" } });
  }) as typeof fetch;

  await approveRun(session, "run_a");
  await cancelRun(session, "run_a");
  expect(calls).toEqual([
    {
      path: "/api/v1/runs/run_a/approve",
      method: "POST",
      body: { reason: "Approved from Takosumi Mobile" },
    },
    { path: "/api/v1/runs/run_a/cancel", method: "POST", body: undefined },
  ]);
});
