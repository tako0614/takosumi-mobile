import {
  createMobileApiClient,
  ensureFreshMobileSession,
  type MobileSession,
  type NativeBridge,
  type WireDecoder,
} from "@takosjp/mobile-kit";
import {
  PUBLIC_SESSION_CAPSULES_RESPONSE_DECODER,
  PUBLIC_SESSION_RUN_RESPONSE_DECODER,
  PUBLIC_SESSION_RUNS_RESPONSE_DECODER,
  PUBLIC_SESSION_WORKSPACES_RESPONSE_DECODER,
  type PublicSessionCapsule,
  type PublicSessionRun,
  type PublicSessionWorkspace,
} from "../../takosumi/contract/public-session-control.ts";
import { productAdapter } from "./product.ts";

const WORKSPACE_PAGE_SIZE = 100;
const CAPSULE_PAGE_SIZE = 100;
const RUN_PAGE_SIZE = 50;
const MAX_LIST_PAGES = 100;
const MAX_WORKSPACES = 1_000;
const MAX_CAPSULES = 5_000;
const MAX_RUNS = 1_000;

export type MobileWorkspace = PublicSessionWorkspace;
export type MobileCapsule = PublicSessionCapsule;
export type MobileCapsuleStatus = MobileCapsule["status"];
export type MobileRun = PublicSessionRun;
export type MobileRunStatus = MobileRun["status"];
export type MobileRunType = MobileRun["type"];

export interface TakosumiMobileHome {
  readonly workspaces: readonly MobileWorkspace[];
  readonly workspace?: MobileWorkspace;
  readonly capsules: readonly MobileCapsule[];
  readonly runs: readonly MobileRun[];
}

export interface LoadHomeOptions {
  readonly preferredWorkspaceId?: string;
}

export async function loadHome(
  session: MobileSession,
  options: LoadHomeOptions = {},
): Promise<TakosumiMobileHome> {
  const workspaces = await listWorkspaces(session);
  const workspace =
    workspaces.find(
      (candidate) => candidate.id === options.preferredWorkspaceId,
    ) ?? workspaces[0];
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
  const [capsules, runs] = await Promise.all([
    collectPages({
      api,
      basePath: `/api/v1/workspaces/${encodedId}/capsules?includeDestroyed=false&limit=${CAPSULE_PAGE_SIZE}`,
      decoder: PUBLIC_SESSION_CAPSULES_RESPONSE_DECODER,
      pageItems: (page) => page.capsules,
      maximumItems: MAX_CAPSULES,
      label: "capsules",
    }),
    collectPages({
      api,
      basePath: `/api/v1/workspaces/${encodedId}/runs?limit=${RUN_PAGE_SIZE}`,
      decoder: PUBLIC_SESSION_RUNS_RESPONSE_DECODER,
      pageItems: (page) => page.runs,
      maximumItems: MAX_RUNS,
      label: "runs",
    }),
  ]);
  return { workspaces, workspace, capsules, runs };
}

export interface MobileRunMutationInput {
  readonly session: MobileSession;
  readonly nativeBridge: NativeBridge;
  readonly run: MobileRun;
  /**
   * Product-owned explicit confirmation + user-presence check. It runs before
   * refresh and before any bearer-authenticated request.
   */
  readonly authorize: () => Promise<boolean>;
}

export interface MobileRunMutationResult {
  readonly run: MobileRun;
  readonly session: MobileSession;
}

export async function approveRun(
  input: MobileRunMutationInput,
): Promise<MobileRunMutationResult> {
  if (input.run.type === "restore") {
    throw new Error("Restore approval requires the dedicated restore flow.");
  }
  if (
    input.run.type !== "plan" &&
    input.run.type !== "destroy_plan"
  ) {
    throw new Error("Only a plan Run can be approved.");
  }
  requireWaitingApproval(input.run);
  return await mutateRun(input, "runs:approve", "approve", {
    reason: "Approved from Takosumi Mobile",
  });
}

export async function approveRestoreRun(
  input: MobileRunMutationInput,
): Promise<MobileRunMutationResult> {
  if (input.run.type !== "restore") {
    throw new Error("The dedicated restore flow accepts only restore Runs.");
  }
  requireWaitingApproval(input.run);
  const details = [
    input.run.backupId ? `backup=${input.run.backupId}` : undefined,
    input.run.restoreStateGeneration === undefined
      ? undefined
      : `generation=${input.run.restoreStateGeneration}`,
  ].filter((detail): detail is string => detail !== undefined);
  return await mutateRun(input, "runs:approve", "approve", {
    reason: `Restore explicitly approved from Takosumi Mobile${
      details.length ? ` (${details.join(", ")})` : ""
    }`,
  });
}

export async function cancelRun(
  input: MobileRunMutationInput,
): Promise<MobileRunMutationResult> {
  if (!canCancelRun(input.run)) {
    throw new Error("Only a not-yet-started plan or apply Run can be cancelled.");
  }
  return await mutateRun(input, "runs:cancel", "cancel");
}

export function canApproveRun(run: MobileRun): boolean {
  return (
    run.status === "waiting_approval" &&
    (run.type === "plan" ||
      run.type === "destroy_plan" ||
      run.type === "restore")
  );
}

export function canCancelRun(run: MobileRun): boolean {
  if (run.type === "plan" || run.type === "destroy_plan") {
    return run.status === "queued" || run.status === "waiting_approval";
  }
  if (run.type === "apply" || run.type === "destroy_apply") {
    return run.status === "queued";
  }
  return false;
}

async function listWorkspaces(
  session: MobileSession,
): Promise<readonly MobileWorkspace[]> {
  return await collectPages({
    api: createMobileApiClient({ session }),
    basePath: `/api/v1/workspaces?limit=${WORKSPACE_PAGE_SIZE}`,
    decoder: PUBLIC_SESSION_WORKSPACES_RESPONSE_DECODER,
    pageItems: (page) => page.workspaces,
    maximumItems: MAX_WORKSPACES,
    label: "workspaces",
  });
}

async function mutateRun(
  input: MobileRunMutationInput,
  requiredScope: "runs:approve" | "runs:cancel",
  action: "approve" | "cancel",
  body?: Readonly<Record<string, string>>,
): Promise<MobileRunMutationResult> {
  if (!(await input.authorize())) {
    throw new Error("端末認証を完了できなかったため、操作を中止しました。");
  }
  const session = await ensureFreshMobileSession({
    adapter: productAdapter,
    nativeBridge: input.nativeBridge,
    session: input.session,
  });
  requireMutationScope(session, requiredScope);
  const response = await createMobileApiClient({ session }).wire(
    `/api/v1/runs/${encodeURIComponent(input.run.id)}/${action}`,
    PUBLIC_SESSION_RUN_RESPONSE_DECODER,
    {
      method: "POST",
      ...(body
        ? {
            headers: { "content-type": "application/json" },
            body: JSON.stringify(body),
          }
        : {}),
    },
  );
  return { run: response.run, session };
}

function requireWaitingApproval(run: MobileRun): void {
  if (run.status !== "waiting_approval") {
    throw new Error("Only a Run awaiting approval can be approved.");
  }
}

function requireMutationScope(
  session: MobileSession,
  required: "runs:approve" | "runs:cancel",
): void {
  if (!session.scope?.trim()) return;
  const scopes = new Set(session.scope.split(/\s+/u).filter(Boolean));
  if (scopes.has(required) || scopes.has("capsules:write")) return;
  throw new Error(`Mobile session lacks the ${required} scope.`);
}

interface CursorPage {
  readonly nextCursor?: string;
}

async function collectPages<Page extends CursorPage, Item>(input: {
  readonly api: ReturnType<typeof createMobileApiClient>;
  readonly basePath: string;
  readonly decoder: WireDecoder<Page>;
  readonly pageItems: (page: Page) => readonly Item[];
  readonly maximumItems: number;
  readonly label: string;
}): Promise<readonly Item[]> {
  const items: Item[] = [];
  const seenCursors = new Set<string>();
  let cursor: string | undefined;
  for (let page = 0; page < MAX_LIST_PAGES; page += 1) {
    const path =
      cursor === undefined
        ? input.basePath
        : `${input.basePath}&cursor=${encodeURIComponent(cursor)}`;
    const body = await input.api.wire(path, input.decoder);
    items.push(...input.pageItems(body));
    const nextCursor = body.nextCursor;
    if (
      items.length > input.maximumItems ||
      (items.length === input.maximumItems && nextCursor)
    ) {
      throw new Error(`${input.label} listing exceeds its safety limit.`);
    }
    if (!nextCursor) return items;
    if (seenCursors.has(nextCursor)) {
      throw new Error(`${input.label} pagination repeated a cursor.`);
    }
    seenCursors.add(nextCursor);
    cursor = nextCursor;
  }
  throw new Error(`${input.label} pagination exceeded its safety limit.`);
}
