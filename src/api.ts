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
import {
  appMayHaveScreens,
  CAPSULE_APP_SCREENS_DECODER,
  type CapsuleAppScreens,
  type LauncherApp,
} from "./app-screens.ts";
import { productAdapter } from "./product.ts";

/**
 * Takosumi API client for one instance's session. Paths are relative to the
 * API root the instance advertised in `/.well-known/takosumi` (`apiBaseUrl`),
 * which mobile-kit only accepts on the instance's own origin; `/api/v1` on the
 * instance is the fallback for sessions saved before it was recorded.
 */
function takosumiApi(session: MobileSession) {
  return createMobileApiClient({ session, apiBasePath: "/api/v1" });
}

const WORKSPACE_PAGE_SIZE = 100;
const APP_SCREEN_CONCURRENCY = 6;
export const MAX_LAUNCHER_APPS = 100;
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
  readonly displayName?: string | Readonly<{ ja: string; en: string }>;
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
  options: LoadHomeOptions = {},
): Promise<TakosumiMobileHome> {
  let apps: readonly MobileApp[];
  try {
    apps = await listApps(session);
  } catch (error) {
    if (
      error instanceof MobileApiError &&
      (error.status === 404 ||
        (error.status === 403 && isAppAccessDenied(error.detail)))
    ) {
      return await loadLegacyHome(session, options);
    }
    throw error;
  }
  const runs = await listAppRuns(session, apps);
  return { apps, workspaces: [], capsules: [], runs };
}

function isAppAccessDenied(detail: string | null): boolean {
  if (detail === "app_access_denied") return true;
  if (!detail) return false;
  // mobile-kit retains structured server error envelopes as raw JSON detail.
  try {
    const body = requireRecord(JSON.parse(detail), "error response");
    return requireRecord(body.error, "error").code === "app_access_denied";
  } catch {
    return false;
  }
}

async function listAppRuns(
  session: MobileSession,
  apps: readonly MobileApp[],
): Promise<readonly MobileRun[]> {
  const api = takosumiApi(session);
  const perAppPageSize = Math.max(
    1,
    Math.min(RUN_PAGE_SIZE, Math.floor(MAX_RUNS / Math.max(1, apps.length))),
  );
  const states = apps.map((app) => ({
    capsuleId: app.capsuleId,
    cursor: undefined as string | undefined,
    pageCount: 0,
    done: false,
  }));
  const runs: MobileRun[] = [];
  const legacyCapsuleIds = new Set<string>();

  for (let offset = 0; offset < states.length; offset += 20) {
    await Promise.all(
      states.slice(offset, offset + 20).map(async (state) => {
        const page = await readAppRunPage(api, state, perAppPageSize);
        state.pageCount += 1;
        if (!page) {
          legacyCapsuleIds.add(state.capsuleId);
          state.done = true;
          return;
        }
        runs.push(
          ...page.runs
            .filter((run) => run.capsuleId === state.capsuleId)
            .slice(0, Math.max(0, MAX_RUNS - runs.length)),
        );
        state.cursor = page.nextCursor;
        state.done = !page.nextCursor;
      }),
    );
  }

  while (runs.length < MAX_RUNS) {
    const pending = states.filter((state) => !state.done);
    if (pending.length === 0) break;
    if (pending.some((state) => state.pageCount >= MAX_LIST_PAGES)) {
      throw new Error("runs pagination exceeded its safety limit.");
    }
    for (let offset = 0; offset < pending.length && runs.length < MAX_RUNS; offset += 20) {
      const batch = pending.slice(offset, offset + 20);
      await Promise.all(
        batch.map(async (state) => {
          const page = await readAppRunPage(
            api,
            state,
            Math.min(perAppPageSize, Math.max(1, MAX_RUNS - runs.length)),
          );
          state.pageCount += 1;
          if (!page) {
            legacyCapsuleIds.add(state.capsuleId);
            state.done = true;
            runs.splice(
              0,
              runs.length,
              ...runs.filter((run) => run.capsuleId !== state.capsuleId),
            );
            return;
          }
          runs.push(
            ...page.runs
              .filter((run) => run.capsuleId === state.capsuleId)
              .slice(0, Math.max(0, MAX_RUNS - runs.length)),
          );
          state.cursor = page.nextCursor;
          state.done = !page.nextCursor;
        }),
      );
    }
  }

  if (runs.length < MAX_RUNS && legacyCapsuleIds.size > 0) {
    const legacyRuns = await listLegacyRunsForApps(
      api,
      legacyCapsuleIds,
      MAX_RUNS - runs.length,
    );
    runs.push(...legacyRuns);
  }
  return runs.sort((a, b) => b.createdAt.localeCompare(a.createdAt));
}

async function readAppRunPage(
  api: ReturnType<typeof createMobileApiClient>,
  state: {
    readonly capsuleId: string;
    readonly cursor: string | undefined;
  },
  limit: number,
): Promise<{ readonly runs: readonly MobileRun[]; readonly nextCursor?: string } | undefined> {
  const path = `/capsules/${encodeURIComponent(state.capsuleId)}/runs?limit=${limit}${state.cursor ? `&cursor=${encodeURIComponent(state.cursor)}` : ""}`;
  try {
    return await api.wire(path, PUBLIC_SESSION_RUNS_RESPONSE_DECODER);
  } catch (error) {
    if (error instanceof MobileApiError && error.status === 404) return undefined;
    throw error;
  }
}

async function listLegacyRunsForApps(
  api: ReturnType<typeof createMobileApiClient>,
  capsuleIds: ReadonlySet<string>,
  maximumItems: number,
): Promise<readonly MobileRun[]> {
  const capsuleIdList = [...capsuleIds];
  const details: Array<{ capsuleId: string; workspaceId: string } | undefined> = [];
  for (let offset = 0; offset < capsuleIdList.length; offset += 20) {
    details.push(
      ...(await Promise.all(
        capsuleIdList.slice(offset, offset + 20).map(async (capsuleId) => {
          try {
            const detail = await api.wire(
              `/capsules/${encodeURIComponent(capsuleId)}`,
              APP_CAPSULE_DETAIL_DECODER,
            );
            if (detail.id !== capsuleId) {
              throw new Error("Capsule detail id does not match the listed app.");
            }
            return detail.workspaceId
              ? { capsuleId, workspaceId: detail.workspaceId }
              : undefined;
          } catch (error) {
            if (error instanceof MobileApiError && error.status === 404) return undefined;
            throw error;
          }
        }),
      )),
    );
  }
  const workspaceApps = new Map<string, Set<string>>();
  for (const detail of details) {
    if (!detail) continue;
    const capsuleIdsForWorkspace = workspaceApps.get(detail.workspaceId) ?? new Set();
    capsuleIdsForWorkspace.add(detail.capsuleId);
    workspaceApps.set(detail.workspaceId, capsuleIdsForWorkspace);
  }
  const runs: MobileRun[] = [];
  for (const [workspaceId, workspaceCapsuleIds] of workspaceApps) {
    const remaining = maximumItems - runs.length;
    if (remaining <= 0) break;
    try {
      runs.push(
        ...(await collectPages({
          api,
          basePath: `/workspaces/${encodeURIComponent(workspaceId)}/runs?limit=${RUN_PAGE_SIZE}`,
          decoder: PUBLIC_SESSION_RUNS_RESPONSE_DECODER,
          pageItems: (page) =>
            page.runs.filter(
              (run) =>
                run.capsuleId !== undefined &&
                workspaceCapsuleIds.has(run.capsuleId),
            ),
          maximumItems: remaining,
          label: "runs",
        })),
      );
    } catch (error) {
      if (!(error instanceof MobileApiError) || error.status !== 404) throw error;
    }
  }
  return runs;
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
  const api = takosumiApi(session);
  const workspaces = knownWorkspaces ?? (await listWorkspaces(session));
  const workspace = workspaces.find(
    (candidate) => candidate.id === workspaceId,
  );
  if (!workspace) throw new Error("Workspace が見つかりません。");
  const encodedId = encodeURIComponent(workspaceId);
  const [capsules, runs] = await Promise.all([
    collectPages({
      api,
      basePath: `/workspaces/${encodedId}/capsules?includeDestroyed=false&limit=${CAPSULE_PAGE_SIZE}`,
      decoder: PUBLIC_SESSION_CAPSULES_RESPONSE_DECODER,
      pageItems: (page) => page.capsules,
      maximumItems: MAX_CAPSULES,
      label: "capsules",
    }),
    collectPages({
      api,
      basePath: `/workspaces/${encodedId}/runs?limit=${RUN_PAGE_SIZE}`,
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
    api: takosumiApi(session),
    basePath: `/apps?limit=${APP_PAGE_SIZE}`,
    decoder: MOBILE_APPS_RESPONSE_DECODER,
    pageItems: (page) => page.apps,
    maximumItems: MAX_APPS,
    label: "apps",
  });
}

export interface AppScreensLoad {
  /** Launchable screens by Capsule id, for apps whose view could be read. */
  readonly screens: ReadonlyMap<string, CapsuleAppScreens>;
  /** Apps whose view failed for a reason other than absence or access. */
  readonly failedCapsuleIds: readonly string[];
}

/**
 * Read each app's launchable screens from the installed-app view
 * (`GET /api/v1/capsules/{capsuleId}/app`). Hosts without the view (404) or
 * apps the session may not read (403) simply have no screens; the home keeps
 * working without them.
 */
export async function loadAppScreens(
  session: MobileSession,
  apps: readonly LauncherApp[],
): Promise<AppScreensLoad> {
  const api = takosumiApi(session);
  const candidates = apps
    .filter((app) => appMayHaveScreens(app.state))
    .slice(0, MAX_LAUNCHER_APPS);
  const screens = new Map<string, CapsuleAppScreens>();
  const failedCapsuleIds: string[] = [];
  let next = 0;
  const worker = async () => {
    while (next < candidates.length) {
      const app = candidates[next++]!;
      try {
        const view = await api.wire(
          `/capsules/${encodeURIComponent(app.capsuleId)}/app`,
          CAPSULE_APP_SCREENS_DECODER,
        );
        if (view.capsuleId !== app.capsuleId) {
          throw new Error("App view id does not match the listed app.");
        }
        screens.set(app.capsuleId, view);
      } catch (error) {
        if (
          error instanceof MobileApiError &&
          (error.status === 404 || error.status === 403)
        ) {
          continue;
        }
        failedCapsuleIds.push(app.capsuleId);
      }
    }
  };
  await Promise.all(
    Array.from(
      { length: Math.min(APP_SCREEN_CONCURRENCY, candidates.length) },
      worker,
    ),
  );
  return { screens, failedCapsuleIds };
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
    api: takosumiApi(session),
    basePath: `/workspaces?limit=${WORKSPACE_PAGE_SIZE}`,
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
        const displayName = decodeAppDisplayName(app.displayName);
        if (role !== "owner" && role !== "member") {
          throw new Error("app.role must be owner or member");
        }
        return {
          capsuleId: requireString(app.capsuleId, "app.capsuleId"),
          name: requireString(app.name, "app.name"),
          ...(displayName === undefined ? {} : { displayName }),
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

function decodeAppDisplayName(value: unknown): MobileApp["displayName"] {
  if (value === undefined) return undefined;
  if (typeof value === "string") return requireString(value, "app.displayName");
  const localized = requireRecord(value, "app.displayName");
  return {
    ja: requireString(localized.ja, "app.displayName.ja"),
    en: requireString(localized.en, "app.displayName.en"),
  };
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
  const response = await takosumiApi(session).wire(
    `/runs/${encodeURIComponent(input.run.id)}/${action}`,
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
