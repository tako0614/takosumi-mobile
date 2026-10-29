import { afterEach, expect, test } from "bun:test";
import type {
  MobileSecureStore,
  MobileSession,
  NativeBridge,
} from "@takosjp/mobile-kit";
import { legacyWorkspaceOptions } from "../src/home-presentation.ts";
import {
  approveRun,
  approveRestoreRun,
  cancelRun,
  loadHome,
  loadWorkspaceHome,
  type MobileCapsule,
  type MobileRun,
  type MobileWorkspace,
} from "../src/api.ts";

const originalFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = originalFetch;
});

const session: MobileSession = {
  hostUrl: "https://operator.example",
  product: "takosumi",
  accessToken: "takat_fixture_mobile_access",
  tokenType: "Bearer",
  scope:
    "workspaces:read capsules:read runs:read runs:approve runs:cancel offline_access",
  createdAt: "2026-07-20T00:00:00.000Z",
};

test("app-scoped Runs paginate without Capsule detail or Workspace lookups", async () => {
  const paths: string[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    paths.push(url.pathname + url.search);
    if (url.pathname === "/api/v1/apps") {
      return Response.json({
        apps: [
          {
            capsuleId: "cap_mobile",
            name: "Takos Office",
            displayName: { ja: "Office", en: "Office" },
            role: "member",
            ownerDisplayName: "Owner",
            state: "active",
            primaryAction: "open",
          },
        ],
      });
    }
    if (url.pathname === "/api/v1/capsules/cap_mobile/runs") {
      if (url.searchParams.get("cursor") === "runs-next") {
        return Response.json({
          runs: [
            run({
              id: "run_older",
              capsuleId: "cap_mobile",
              createdAt: "2026-07-19T00:00:00.000Z",
            }),
          ],
        });
      }
      return Response.json({
        runs: [
          run({
            id: "run_newer",
            capsuleId: "cap_mobile",
            createdAt: "2026-07-20T00:00:00.000Z",
          }),
        ],
        nextCursor: "runs-next",
      });
    }
    throw new Error(`Unexpected request: ${url.pathname}`);
  }) as typeof fetch;

  const home = await loadHome(session);
  expect(home.apps.map((app) => app.capsuleId)).toEqual(["cap_mobile"]);
  expect(home.apps[0]?.displayName).toEqual({ ja: "Office", en: "Office" });
  expect(home.capsules).toEqual([]);
  expect(home.runs.map((item) => item.id)).toEqual(["run_newer", "run_older"]);
  expect(paths).toEqual([
    "/api/v1/apps?limit=100",
    "/api/v1/capsules/cap_mobile/runs?limit=50",
    "/api/v1/capsules/cap_mobile/runs?limit=50&cursor=runs-next",
  ]);
});

test("a missing apps route falls back to the legacy Workspace API", async () => {
  const paths: string[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    paths.push(url.pathname);
    if (url.pathname === "/api/v1/apps") {
      return new Response(null, { status: 404 });
    }
    if (url.pathname === "/api/v1/workspaces") {
      return Response.json({ workspaces: [] });
    }
    throw new Error(`Unexpected request: ${url.pathname}`);
  }) as typeof fetch;
  const legacyHome = await loadHome(session);
  expect(legacyHome.apps).toEqual([]);
  expect(legacyHome.workspaces).toEqual([]);
  expect(paths).toEqual(["/api/v1/apps", "/api/v1/workspaces"]);

  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    paths.push(url.pathname);
    if (url.pathname === "/api/v1/apps") {
      return Response.json({ message: "unavailable" }, { status: 503 });
    }
    throw new Error(`Unexpected fallback request: ${url.pathname}`);
  }) as typeof fetch;
  await expect(loadHome(session)).rejects.toThrow("503 /api/v1/apps");
  expect(paths.at(-1)).toBe("/api/v1/apps");
});

test("legacy fallback exposes every Workspace and can select the second one", async () => {
  const paths: string[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    paths.push(url.pathname);
    if (url.pathname === "/api/v1/apps") {
      return new Response(null, { status: 404 });
    }
    if (url.pathname === "/api/v1/workspaces") {
      return Response.json({
        workspaces: [workspace(), workspace({ id: "ws_b", displayName: "B" })],
      });
    }
    if (url.pathname.endsWith("/capsules")) {
      return Response.json({ capsules: [] });
    }
    if (url.pathname.endsWith("/runs")) return Response.json({ runs: [] });
    throw new Error(`Unexpected request: ${url.pathname}`);
  }) as typeof fetch;

  const first = await loadHome(session);
  const options = legacyWorkspaceOptions(first);
  expect(options.map((item) => item.id)).toEqual(["ws_a", "ws_b"]);
  const second = await loadWorkspaceHome(session, options[1]!.id, options);
  expect(second.workspace?.id).toBe("ws_b");
  expect(paths.filter((path) => path === "/api/v1/workspaces")).toHaveLength(1);
  expect(paths).toContain("/api/v1/workspaces/ws_b/capsules");
  expect(paths).toContain("/api/v1/workspaces/ws_b/runs");
});

test.each([
  ["the canonical nested error", {
    code: "app_access_denied",
    message: "App access is required.",
    requestId: "00000000-0000-4000-8000-000000000118",
  }],
  ["the legacy flat error", "app_access_denied"],
] as const)("an opaque mobile token uses the legacy home once after %s", async (_label, error) => {
  const paths: string[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    paths.push(url.pathname);
    if (url.pathname === "/api/v1/apps") {
      return Response.json({ error }, { status: 403 });
    }
    if (url.pathname === "/api/v1/workspaces") {
      return Response.json({ workspaces: [workspace(), workspace({ id: "ws_b" })] });
    }
    if (
      url.pathname === "/api/v1/workspaces/ws_a/capsules" ||
      url.pathname === "/api/v1/workspaces/ws_b/capsules"
    ) {
      return Response.json({ capsules: [capsule()] });
    }
    if (
      url.pathname === "/api/v1/workspaces/ws_a/runs" ||
      url.pathname === "/api/v1/workspaces/ws_b/runs"
    ) {
      return Response.json({ runs: [] });
    }
    throw new Error(`Unexpected request: ${url.pathname}`);
  }) as typeof fetch;

  const home = await loadHome(session);
  expect(home.workspace?.id).toBe("ws_a");
  expect(home.capsules).toHaveLength(1);
  expect(paths).toEqual([
    "/api/v1/apps",
    "/api/v1/workspaces",
    "/api/v1/workspaces/ws_a/capsules",
    "/api/v1/workspaces/ws_a/runs",
  ]);

  paths.length = 0;
  const selected = await loadHome(session, { preferredWorkspaceId: "ws_b" });
  expect(selected.workspace?.id).toBe("ws_b");
  expect(paths).toEqual([
    "/api/v1/apps",
    "/api/v1/workspaces",
    "/api/v1/workspaces/ws_b/capsules",
    "/api/v1/workspaces/ws_b/runs",
  ]);
});

test("other apps errors do not trigger legacy fallback", async () => {
  for (const error of [
    "forbidden",
    "insufficient_scope",
    "permission_denied",
    "{invalid-json",
    { code: "forbidden", message: "app_access_denied" },
    { message: "app_access_denied" },
    { code: 403, message: "App access is required." },
    [{ code: "app_access_denied" }],
    null,
  ]) {
    const paths: string[] = [];
    globalThis.fetch = (async (input: RequestInfo | URL) => {
      paths.push(new URL(String(input)).pathname);
      return Response.json({ error }, { status: 403 });
    }) as typeof fetch;
    await expect(loadHome(session)).rejects.toThrow(
      "403 /api/v1/apps",
    );
    expect(paths).toEqual(["/api/v1/apps"]);
  }

  for (const status of [401, 429, 500, 503]) {
    const paths: string[] = [];
    globalThis.fetch = (async (input: RequestInfo | URL) => {
      paths.push(new URL(String(input)).pathname);
      return Response.json({
        error: { code: "app_access_denied", message: "App access is required." },
      }, { status });
    }) as typeof fetch;
    await expect(loadHome(session)).rejects.toThrow(`${status} /api/v1/apps`);
    expect(paths).toEqual(["/api/v1/apps"]);
  }
});

test("legacy authorization and network failures are still surfaced", async () => {
  const paths: string[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    paths.push(url.pathname);
    return Response.json({
      error: { code: "app_access_denied", message: "App access is required." },
    }, { status: 403 });
  }) as typeof fetch;
  await expect(loadHome(session)).rejects.toThrow("403 /api/v1/workspaces");
  expect(paths).toEqual(["/api/v1/apps", "/api/v1/workspaces"]);

  paths.length = 0;
  globalThis.fetch = (async (input: RequestInfo | URL): Promise<Response> => {
    const url = new URL(String(input));
    paths.push(url.pathname);
    if (url.pathname === "/api/v1/apps") {
      return Response.json({
        error: { code: "app_access_denied", message: "App access is required." },
      }, { status: 403 });
    }
    throw new Error("Legacy network unavailable");
  }) as typeof fetch;
  await expect(loadHome(session)).rejects.toThrow("Legacy network unavailable");
  expect(paths).toEqual(["/api/v1/apps", "/api/v1/workspaces"]);

  paths.length = 0;
  globalThis.fetch = (async (input: RequestInfo | URL): Promise<Response> => {
    paths.push(new URL(String(input)).pathname);
    throw new Error("Network unavailable");
  }) as typeof fetch;
  await expect(loadHome(session)).rejects.toThrow("Network unavailable");
  expect(paths).toEqual(["/api/v1/apps"]);
});

test("per-app Run 404 falls back to Workspace Runs filtered to listed apps", async () => {
  const paths: string[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    paths.push(url.pathname + url.search);
    if (url.pathname === "/api/v1/apps") {
      return Response.json({
        apps: [
          {
            capsuleId: "cap_fallback",
            name: "Legacy app",
            role: "owner",
            ownerDisplayName: "Owner",
            state: "active",
            primaryAction: "open",
          },
          {
            capsuleId: "cap_current",
            name: "Current app",
            displayName: "Current app label",
            role: "owner",
            ownerDisplayName: "Owner",
            state: "active",
            primaryAction: "open",
          },
        ],
      });
    }
    if (url.pathname === "/api/v1/capsules/cap_fallback/runs") {
      return new Response(null, { status: 404 });
    }
    if (url.pathname === "/api/v1/capsules/cap_current/runs") {
      return Response.json({ runs: [] });
    }
    if (url.pathname === "/api/v1/capsules/cap_fallback") {
      return Response.json({
        capsule: { id: "cap_fallback", workspaceId: "ws_legacy" },
      });
    }
    if (url.pathname === "/api/v1/workspaces/ws_legacy/runs") {
      return Response.json({
        runs: [
          run({ id: "run_visible", capsuleId: "cap_fallback" }),
          run({ id: "run_unrelated", capsuleId: "cap_hidden" }),
        ],
      });
    }
    throw new Error(`Unexpected request: ${url.pathname}`);
  }) as typeof fetch;

  const home = await loadHome(session);
  expect(home.runs.map((item) => item.id)).toEqual(["run_visible"]);
  expect(
    home.apps.find((app) => app.capsuleId === "cap_current")?.displayName,
  ).toBe("Current app label");
  expect(paths).toContain("/api/v1/capsules/cap_fallback");
  expect(paths).toContain("/api/v1/workspaces/ws_legacy/runs?limit=50");
  expect(paths).toContain("/api/v1/capsules/cap_current/runs?limit=50");
  expect(paths).not.toContain("/api/v1/capsules/cap_current");
});

test("a per-app Run 404 without detail Workspace skips optional history", async () => {
  const paths: string[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    paths.push(url.pathname);
    if (url.pathname === "/api/v1/apps") {
      return Response.json({
        apps: [
          {
            capsuleId: "cap_no_workspace",
            name: "No Workspace app",
            role: "member",
            ownerDisplayName: "Owner",
            state: "active",
            primaryAction: "open",
          },
        ],
      });
    }
    if (url.pathname === "/api/v1/capsules/cap_no_workspace/runs") {
      return new Response(null, { status: 404 });
    }
    if (url.pathname === "/api/v1/capsules/cap_no_workspace") {
      return Response.json({ capsule: { id: "cap_no_workspace" } });
    }
    throw new Error(`Unexpected request: ${url.pathname}`);
  }) as typeof fetch;

  const home = await loadHome(session);
  expect(home.runs).toEqual([]);
  expect(paths).toContain("/api/v1/capsules/cap_no_workspace");
  expect(paths.some((path) => path.startsWith("/api/v1/workspaces/"))).toBe(false);
});

test("authorization failures from per-app Run listing are not masked", async () => {
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    if (url.pathname === "/api/v1/apps") {
      return Response.json({
        apps: [
          {
            capsuleId: "cap_forbidden",
            name: "Forbidden app",
            role: "member",
            ownerDisplayName: "Owner",
            state: "active",
            primaryAction: "open",
          },
        ],
      });
    }
    return Response.json({ message: "forbidden" }, { status: 403 });
  }) as typeof fetch;

  await expect(loadHome(session)).rejects.toThrow("403 /api/v1/capsules/cap_forbidden/runs");
});

test("legacy Run listing uses only a Workspace id from Capsule detail", async () => {
  const paths: string[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    paths.push(url.pathname + url.search);
    if (url.pathname === "/api/v1/apps") {
      return Response.json({
        apps: [
          {
            capsuleId: "cap_legacy",
            name: "Legacy app",
            role: "owner",
            ownerDisplayName: "Owner",
            state: "active",
            primaryAction: "open",
          },
        ],
      });
    }
    if (url.pathname === "/api/v1/capsules/cap_legacy/runs") {
      return new Response(null, { status: 404 });
    }
    if (url.pathname === "/api/v1/capsules/cap_legacy") {
      return Response.json({
        capsule: { id: "cap_legacy", workspaceId: "ws_from_detail" },
      });
    }
    if (url.pathname === "/api/v1/workspaces/ws_from_detail/runs") {
      return Response.json({ runs: [] });
    }
    throw new Error(`Unexpected request: ${url.pathname}`);
  }) as typeof fetch;

  const home = await loadHome(session);
  expect(home.runs).toEqual([]);
  expect(paths).toEqual([
    "/api/v1/apps?limit=100",
    "/api/v1/capsules/cap_legacy/runs?limit=50",
    "/api/v1/capsules/cap_legacy",
    "/api/v1/workspaces/ws_from_detail/runs?limit=50",
  ]);
});

test("home follows opaque pages and restores the preferred Workspace", async () => {
  const paths: string[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input));
    paths.push(url.pathname + url.search);
    expect(new Headers(init?.headers).get("authorization")).toBe(
      `Bearer ${session.accessToken}`,
    );
    if (url.pathname === "/api/v1/apps") {
      return new Response(null, { status: 404 });
    }
    if (url.pathname === "/api/v1/workspaces") {
      if (url.searchParams.get("cursor") === "workspace-next") {
        return Response.json({
          workspaces: [
            workspace({
              id: "ws_b",
              handle: "b",
              displayName: "B",
              type: "organization",
              ownerUserId: "acct_b",
            }),
          ],
        });
      }
      return Response.json({
        workspaces: [
          workspace({
            id: "ws_a",
            handle: "a",
            displayName: "A",
            type: "personal",
            ownerUserId: "acct_a",
          }),
        ],
        nextCursor: "workspace-next",
      });
    }
    if (url.pathname.endsWith("/capsules")) {
      if (url.searchParams.get("cursor") === "capsule-next") {
        return Response.json({
          capsules: [capsule({ id: "cap_b", name: "Takos Office" })],
        });
      }
      return Response.json({
        capsules: [capsule({ id: "cap_a", name: "Takos" })],
        nextCursor: "capsule-next",
      });
    }
    if (url.searchParams.get("cursor") === "run-next") {
      return Response.json({ runs: [run({ id: "run_b", status: "failed" })] });
    }
    return Response.json({
      runs: [run({ id: "run_a", status: "running" })],
      nextCursor: "run-next",
    });
  }) as typeof fetch;

  const home = await loadHome(session, { preferredWorkspaceId: "ws_b" });
  expect(home.workspace?.id).toBe("ws_b");
  expect(home.workspace?.type).toBe("organization");
  expect(home.workspace?.ownerUserId).toBe("acct_b");
  expect(home.capsules).toHaveLength(2);
  expect(home.runs).toHaveLength(2);
  expect(paths.sort()).toEqual([
    "/api/v1/apps?limit=100",
    "/api/v1/workspaces/ws_b/capsules?includeDestroyed=false&limit=100",
    "/api/v1/workspaces/ws_b/capsules?includeDestroyed=false&limit=100&cursor=capsule-next",
    "/api/v1/workspaces/ws_b/runs?limit=50",
    "/api/v1/workspaces/ws_b/runs?limit=50&cursor=run-next",
    "/api/v1/workspaces?limit=100",
    "/api/v1/workspaces?limit=100&cursor=workspace-next",
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
    workspace({
      id: "ws_b",
      handle: "b",
      displayName: "B",
      type: "organization",
      ownerUserId: "acct_b",
    }),
  ];
  const home = await loadWorkspaceHome(session, "ws_b", workspaces);
  expect(home.workspace?.displayName).toBe("B");
  expect(workspaceListRequests).toBe(0);
});

test("legacy team owner kind is rejected instead of being persisted", async () => {
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    if (url.pathname === "/api/v1/apps") {
      return new Response(null, { status: 404 });
    }
    return Response.json({
      workspaces: [
        {
          id: "ws_legacy",
          handle: "legacy",
          displayName: "Legacy",
          type: "team",
          ownerUserId: "acct_a",
          createdAt: "2026-07-20T00:00:00.000Z",
          updatedAt: "2026-07-20T00:00:00.000Z",
        },
      ],
    });
  }) as typeof fetch;

  await expect(loadHome(session)).rejects.toThrow(
    "workspace.type must be personal or organization",
  );
});

test("home rejects a Capsule missing canonical owner fields", async () => {
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    const url = new URL(String(input));
    if (url.pathname.endsWith("/capsules")) {
      const invalid = capsule();
      const { projectId: _omitted, ...withoutProject } = invalid;
      return Response.json({ capsules: [withoutProject] });
    }
    return Response.json({ runs: [] });
  }) as typeof fetch;

  await expect(
    loadWorkspaceHome(session, "ws_a", [workspace()]),
  ).rejects.toThrow("capsule.projectId must be a non-empty string");
});

test("approval and cancellation authorize, refresh, then use narrow Run routes", async () => {
  const calls: Array<{ path: string; method?: string; body?: unknown }> = [];
  const order: string[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input));
    if (url.pathname === "/.well-known/openid-configuration") {
      order.push("metadata");
      return Response.json({
        issuer: "https://operator.example",
        authorization_endpoint: "https://operator.example/oauth/authorize",
        token_endpoint: "https://operator.example/oauth/token",
      });
    }
    if (url.pathname === "/oauth/token") {
      order.push("refresh");
      return Response.json({
        access_token: "fresh-access",
        token_type: "Bearer",
        expires_in: 3600,
        scope: session.scope,
      });
    }
    order.push("mutation");
    calls.push({
      path: url.pathname,
      method: init?.method,
      body: init?.body ? await new Response(init.body).json() : undefined,
    });
    expect(new Headers(init?.headers).get("authorization")).toBe(
      "Bearer fresh-access",
    );
    return Response.json({ run: run({ status: "succeeded" }) });
  }) as typeof fetch;

  const expired = {
    ...session,
    oidcIssuer: "https://operator.example",
    oidcClientId: "takosumi-mobile-operator",
    refreshToken: "refresh",
    expiresAt: "2026-07-20T00:00:00.000Z",
  };
  const bridge = memoryBridge();
  const approve = await approveRun({
    session: expired,
    nativeBridge: bridge,
    run: run({ type: "plan", status: "waiting_approval" }),
    authorize: async () => {
      order.push("authorize");
      return true;
    },
  });
  const cancelled = await cancelRun({
    session: expired,
    nativeBridge: bridge,
    run: run({ type: "apply", status: "queued" }),
    authorize: async () => {
      order.push("authorize");
      return true;
    },
  });
  expect(approve.session.accessToken).toBe("fresh-access");
  expect(cancelled.session.accessToken).toBe("fresh-access");
  expect(calls).toEqual([
    {
      path: "/api/v1/runs/run_a/approve",
      method: "POST",
      body: { reason: "Approved from Takosumi Mobile" },
    },
    { path: "/api/v1/runs/run_a/cancel", method: "POST", body: undefined },
  ]);
  expect(order).toEqual([
    "authorize",
    "metadata",
    "refresh",
    "mutation",
    "authorize",
    "metadata",
    "refresh",
    "mutation",
  ]);
});

test("Run mutations reject an incomplete canonical response", async () => {
  globalThis.fetch = (async (_input: RequestInfo | URL) =>
    Response.json({
      run: {
        id: "run_a",
        workspaceId: "ws_a",
        type: "plan",
        status: "succeeded",
        createdAt: "2026-07-20T00:00:00.000Z",
      },
    })) as typeof fetch;

  await expect(
    approveRun({
      session,
      nativeBridge: memoryBridge(),
      run: run(),
      authorize: async () => true,
    }),
  ).rejects.toThrow("run.createdBy must be a non-empty string");
});

test("restore cannot pass through generic approval and uses its dedicated reason", async () => {
  let fetches = 0;
  globalThis.fetch = (async (_input: RequestInfo | URL) => {
    fetches += 1;
    return Response.json({ run: run({ type: "restore", status: "queued" }) });
  }) as typeof fetch;
  const restore = run({
    type: "restore",
    status: "waiting_approval",
    backupId: "backup_1",
    restoreStateGeneration: 8,
  });
  const input = {
    session,
    nativeBridge: memoryBridge(),
    run: restore,
    authorize: async () => true,
  };

  await expect(approveRun(input)).rejects.toThrow("dedicated restore flow");
  expect(fetches).toBe(0);

  await approveRestoreRun(input);
  expect(fetches).toBe(1);
});

test("mutation denial happens before refresh or bearer access", async () => {
  let fetches = 0;
  globalThis.fetch = (async (_input: RequestInfo | URL) => {
    fetches += 1;
    throw new Error("must not fetch");
  }) as unknown as typeof fetch;

  await expect(
    cancelRun({
      session,
      nativeBridge: memoryBridge(),
      run: run({ type: "apply", status: "queued" }),
      authorize: async () => false,
    }),
  ).rejects.toThrow("端末認証");
  expect(fetches).toBe(0);
});

function run(
  input: Partial<MobileRun> = {},
): MobileRun {
  return {
    id: "run_a",
    workspaceId: "ws_a",
    capsuleId: "cap_a",
    type: "plan",
    status: "waiting_approval",
    createdBy: "acct_a",
    createdAt: "2026-07-20T00:00:00.000Z",
    ...input,
  };
}

function workspace(
  input: Partial<MobileWorkspace> = {},
): MobileWorkspace {
  return {
    id: "ws_a",
    handle: "workspace-a",
    displayName: "Workspace A",
    type: "personal",
    ownerUserId: "acct_a",
    createdAt: "2026-07-20T00:00:00.000Z",
    updatedAt: "2026-07-20T00:00:00.000Z",
    ...input,
  };
}

function capsule(
  input: Partial<MobileCapsule> = {},
): MobileCapsule {
  return {
    id: "cap_a",
    workspaceId: "ws_b",
    projectId: "project_a",
    name: "Capsule A",
    slug: "capsule-a",
    sourceId: "source_a",
    installConfigId: "config_a",
    environment: "production",
    currentStateGeneration: 1,
    status: "active",
    createdAt: "2026-07-20T00:00:00.000Z",
    updatedAt: "2026-07-20T00:00:00.000Z",
    ...input,
  };
}

function memoryBridge(): NativeBridge {
  const values = new Map<string, string>();
  const store: MobileSecureStore = {
    kind: "secure",
    get: async (key) => values.get(key),
    set: async (key, value) => {
      values.set(key, value);
    },
    delete: async (key) => {
      values.delete(key);
    },
  };
  return {
    capabilities: {
      launchPayload: false,
      launchPayloadEvents: false,
      externalBrowser: false,
      inAppBrowser: false,
      qrScanner: false,
      localNotifications: false,
      pushNotifications: false,
      biometricAuth: false,
      callIntent: false,
      clipboardText: false,
      secureStorage: true,
      persistentStorage: false,
    },
    secureStore: store,
    getLaunchPayload: async () => undefined,
    openExternalUrl: async () => undefined,
  };
}
