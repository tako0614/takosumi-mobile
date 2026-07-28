import { afterEach, expect, test } from "bun:test";
import type {
  MobileSecureStore,
  MobileSession,
  NativeBridge,
} from "@takosjp/mobile-kit";
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
  accessToken: "mobile-access",
  tokenType: "Bearer",
  scope:
    "workspaces:read capsules:read runs:read runs:approve runs:cancel offline_access",
  createdAt: "2026-07-20T00:00:00.000Z",
};

test("home follows opaque pages and restores the preferred Workspace", async () => {
  const paths: string[] = [];
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input));
    paths.push(url.pathname + url.search);
    expect(new Headers(init?.headers).get("authorization")).toBe(
      "Bearer mobile-access",
    );
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
  globalThis.fetch = (async (_input: RequestInfo | URL) =>
    Response.json({
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
    })) as typeof fetch;

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
