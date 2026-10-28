import {
  createMobileApiClient,
  ensureFreshMobileSession,
  MobileApiError,
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
const APP_PAGE_SIZE = 100;
const CAPSULE_PAGE_SIZE = 100;
const RUN_PAGE_SIZE = 50;
const MAX_LIST_PAGES = 100;
const MAX_WORKSPACES = 1_000;
const MAX_APPS = 1_000;
const MAX_CAPSULES = 5_000;
const MAX_RUNS = 1_000;

export type MobileWorkspace = PublicSessionWorkspace;
export type MobileCapsule = PublicSessionCapsule;
export type MobileCapsuleStatus = MobileCapsule["status"];
export type MobileRun = PublicSessionRun;
export type MobileRunStatus = MobileRun["status"];
export type MobileRunType = MobileRun["type"];

export interface MobileApp {
  readonly capsuleId: string;
  readonly name: string;
  readonly displayName?: string;
  readonly iconUrl?: string;
  readonly role: "owner" | "member";
  readonly ownerDisplayName: string;
  readonly state: string;
  readonly primaryAction: string;
}

export interface TakosumiMobileHome {
  readonly apps: readonly MobileApp[];
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
  _options: LoadHomeOptions = {},
): Promise<TakosumiMobileHome> {
  let apps: readonly MobileApp[];
  try {
    apps = await listApps(session);
  } catch (error) {
    if (error instanceof MobileApiError && error.status === 404) {
      return await loadLegacyHome(session, _options);
    }
    throw error;
  }
  const api = createMobileApiClient({ session });
  const capsuleDetails: MobileCapsuleDetail[] = [];
  for (let offset = 0; offset < apps.length; offset += 20) {
    const details = await Promise.all(
      apps.slice(offset, offset + 20).map(async (app) => {
        const detail = await api.wire(
          `/api/v1/capsules/${encodeURIComponent(app.capsuleId)}`,
          APP_CAPSULE_DETAIL_DECODER,
        );
        if (detail.id !== app.capsuleId) {
          throw new Error("Capsule detail id does not match the listed app.");
        }
        return detail;
      }),
    );
    capsuleDetails.push(...details);
  }
  const workspaceIds = [...new Set(
    capsuleDetails.flatMap((capsule) =>
      capsule.workspaceId ? [capsule.workspaceId] : [],
    ),
  )];
  const runs: MobileRun[] = [];
  for (const [index, workspaceId] of workspaceIds.entries()) {
    const remaining = MAX_RUNS - runs.length;
    if (remaining <= 0) {
      throw new Error("runs listing exceeds its safety limit.");
    }
    try {
      runs.push(
        ...(await collectPages({
          api,
          basePath: `/api/v1/workspaces/${encodeURIComponent(workspaceId)}/runs?limit=${RUN_PAGE_SIZE}`,
          decoder: PUBLIC_SESSION_RUNS_RESPONSE_DECODER,
          pageItems: (page) => page.runs,
          maximumItems: remaining,
          label: "runs",
        })),
      );
    } catch (error) {
      if (!(error instanceof MobileApiError) || error.status !== 404) throw error;
      if (index === workspaceIds.length - 1) continue;
    }
  }
  return { apps, workspaces: [], capsules: [], runs };
}

async function loadLegacyHome(
  session: MobileSession,
  options: LoadHomeOptions,
): Promise<TakosumiMobileHome> {
  const workspaces = await listWorkspaces(session);
  const workspace =
    workspaces.find(
      (candidate) => candidate.id === options.preferredWorkspaceId,
    ) ?? workspaces[0];
  if (!workspace) return { apps: [], workspaces, capsules: [], runs: [] };
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
  return { apps: [], workspaces, workspace, capsules, runs };
}

async function listApps(session: MobileSession): Promise<readonly MobileApp[]> {
  return await collectPages({
    api: createMobileApiClient({ session }),
    basePath: `/api/v1/apps?limit=${APP_PAGE_SIZE}`,
    decoder: MOBILE_APPS_RESPONSE_DECODER,
    pageItems: (page) => page.apps,
    maximumItems: MAX_APPS,
    label: "apps",
  });
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

interface MobileAppsPage {
  readonly apps: readonly MobileApp[];
  readonly nextCursor?: string;
}

interface MobileCapsuleDetail {
  readonly id: string;
  readonly workspaceId?: string;
}

const MOBILE_APPS_RESPONSE_DECODER: WireDecoder<MobileAppsPage> = {
  document: "Takosumi app list",
  decode(value) {
    const body = requireRecord(value, "document");
    const nextCursor = optionalString(body.nextCursor, "nextCursor");
    return {
      apps: requireArray(body.apps, "apps").map((item) => {
        const app = requireRecord(item, "app");
        const role = app.role;
        if (role !== "owner" && role !== "member") {
          throw new Error("app.role must be owner or member");
        }
        return {
          capsuleId: requireString(app.capsuleId, "app.capsuleId"),
          name: requireString(app.name, "app.name"),
          ...(optionalString(app.displayName, "app.displayName") === undefined
            ? {}
            : { displayName: app.displayName as string }),
          ...(optionalString(app.iconUrl, "app.iconUrl") === undefined
            ? {}
            : { iconUrl: app.iconUrl as string }),
          role,
          ownerDisplayName: requireString(
            app.ownerDisplayName,
            "app.ownerDisplayName",
          ),
          state: requireString(app.state, "app.state"),
          primaryAction: requireString(
            app.primaryAction,
            "app.primaryAction",
          ),
        };
      }),
      ...(nextCursor === undefined ? {} : { nextCursor }),
    };
  },
};

const APP_CAPSULE_DETAIL_DECODER: WireDecoder<MobileCapsuleDetail> = {
  document: "Takosumi app Capsule detail",
  decode(value) {
    const body = requireRecord(value, "document");
    const capsule = requireRecord(body.capsule, "capsule");
    return {
      id: requireString(capsule.id, "capsule.id"),
      ...(optionalString(capsule.workspaceId, "capsule.workspaceId") ===
      undefined
        ? {}
        : { workspaceId: capsule.workspaceId as string }),
    };
  },
};

function requireRecord(value: unknown, label: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  return value as Record<string, unknown>;
}

function requireArray(value: unknown, label: string): readonly unknown[] {
  if (!Array.isArray(value)) throw new Error(`${label} must be an array`);
  return value;
}

function requireString(value: unknown, label: string): string {
  if (typeof value !== "string" || !value.trim()) {
    throw new Error(`${label} must be a non-empty string`);
  }
  return value;
}

function optionalString(value: unknown, label: string): string | undefined {
  if (value === undefined) return undefined;
  return requireString(value, label);
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
