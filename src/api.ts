import { createMobileApiClient, type MobileSession } from "@takosjp/mobile-kit";

export interface MobileWorkspace {
  readonly id: string;
  readonly handle: string;
  readonly displayName: string;
  readonly type: "personal" | "team";
}

export type MobileCapsuleStatus =
  "pending" | "active" | "stale" | "error" | "disabled" | "destroyed";

export interface MobileCapsule {
  readonly id: string;
  readonly workspaceId: string;
  readonly name: string;
  readonly slug: string;
  readonly environment: string;
  readonly status: MobileCapsuleStatus;
  readonly updatedAt: string;
}

export type MobileRunStatus =
  | "queued"
  | "running"
  | "waiting_approval"
  | "succeeded"
  | "failed"
  | "cancelled"
  | "expired";

export type MobileRunType =
  | "source_sync"
  | "compatibility_check"
  | "plan"
  | "apply"
  | "destroy_plan"
  | "destroy_apply"
  | "drift_check"
  | "backup"
  | "restore";

export interface MobileRun {
  readonly id: string;
  readonly workspaceId: string;
  readonly capsuleId?: string;
  readonly type: MobileRunType;
  readonly status: MobileRunStatus;
  readonly requiresApproval?: boolean;
  readonly summary?: {
    readonly add?: number;
    readonly change?: number;
    readonly destroy?: number;
  };
  readonly errorCode?: string;
  readonly createdAt: string;
  readonly finishedAt?: string;
}

export interface TakosumiMobileHome {
  readonly workspaces: readonly MobileWorkspace[];
  readonly workspace?: MobileWorkspace;
  readonly capsules: readonly MobileCapsule[];
  readonly runs: readonly MobileRun[];
}

export async function loadHome(
  session: MobileSession,
): Promise<TakosumiMobileHome> {
  const workspaces = await listWorkspaces(session);
  const workspace = workspaces[0];
  if (!workspace) return { workspaces, capsules: [], runs: [] };
  return await loadWorkspaceHome(session, workspace.id, workspaces);
}

export async function loadWorkspaceHome(
  session: MobileSession,
  workspaceId: string,
  knownWorkspaces?: readonly MobileWorkspace[],
): Promise<TakosumiMobileHome> {
  const api = createMobileApiClient({ session });
  const workspaces = knownWorkspaces ?? (await listWorkspaces(session));
  const workspace = workspaces.find(
    (candidate) => candidate.id === workspaceId,
  );
  if (!workspace) throw new Error("Workspace が見つかりません。");
  const encodedId = encodeURIComponent(workspaceId);
  const [capsulePage, runPage] = await Promise.all([
    api.json<{ capsules?: readonly MobileCapsule[] }>(
      `/api/v1/workspaces/${encodedId}/capsules?includeDestroyed=false`,
    ),
    api.json<{ runs?: readonly MobileRun[] }>(
      `/api/v1/workspaces/${encodedId}/runs?limit=30`,
    ),
  ]);
  return {
    workspaces,
    workspace,
    capsules: capsulePage.capsules ?? [],
    runs: runPage.runs ?? [],
  };
}

export async function approveRun(
  session: MobileSession,
  runId: string,
): Promise<MobileRun> {
  const body = await createMobileApiClient({ session }).json<{
    run: MobileRun;
  }>(`/api/v1/runs/${encodeURIComponent(runId)}/approve`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ reason: "Approved from Takosumi Mobile" }),
  });
  return body.run;
}

export async function cancelRun(
  session: MobileSession,
  runId: string,
): Promise<MobileRun> {
  const body = await createMobileApiClient({ session }).json<{
    run: MobileRun;
  }>(`/api/v1/runs/${encodeURIComponent(runId)}/cancel`, {
    method: "POST",
  });
  return body.run;
}

async function listWorkspaces(
  session: MobileSession,
): Promise<readonly MobileWorkspace[]> {
  const body = await createMobileApiClient({ session }).json<{
    workspaces?: readonly MobileWorkspace[];
  }>("/api/v1/workspaces");
  return body.workspaces ?? [];
}
