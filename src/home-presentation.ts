import type {
  MobileApp,
  MobileRun,
  MobileRunStatus,
  MobileRunType,
  MobileWorkspace,
  TakosumiMobileHome,
} from "./api.ts";
import { canApproveRun } from "./api.ts";
import type { LauncherApp } from "./app-screens.ts";

export function mobileHomeTitle(_home?: TakosumiMobileHome): string {
  return "Takosumi";
}

/**
 * Apps for the launcher: the per-app list when the host serves it, else the
 * legacy Workspace's Capsules (production hosts before TASK-0118).
 */
export function launcherAppsFromHome(
  home: TakosumiMobileHome | undefined,
  locale?: string,
): readonly LauncherApp[] {
  if (!home) return [];
  if (home.apps.length > 0) {
    return home.apps.map((app) => ({
      capsuleId: app.capsuleId,
      label: appDisplayName(app, locale),
      ...(app.iconUrl === undefined ? {} : { iconUrl: app.iconUrl }),
      state: app.state,
    }));
  }
  return home.capsules.map((capsule) => ({
    capsuleId: capsule.id,
    label: capsule.name,
    state: capsule.status,
  }));
}

export function legacyWorkspaceOptions(
  home: TakosumiMobileHome | undefined,
): readonly MobileWorkspace[] {
  return home?.workspace ? home.workspaces : [];
}

export function appDisplayName(
  app: Pick<MobileApp, "name" | "displayName">,
  locale = typeof navigator === "undefined" ? "ja-JP" : navigator.language,
): string {
  if (typeof app.displayName === "string") return app.displayName;
  if (!app.displayName) return app.name;
  return locale.toLowerCase().split(/[-_]/u)[0] === "ja"
    ? app.displayName.ja
    : app.displayName.en;
}

export function appRoleLabel(role: MobileApp["role"]): string {
  return role === "owner" ? "所有者" : "メンバー";
}

export function appStateLabel(state: MobileApp["state"]): string {
  const labels: Readonly<Record<string, string>> = {
    installing: "インストール中",
    installed: "稼働中",
    updating: "更新中",
    needs_attention: "要対応",
    stopped: "停止中",
    uninstall_scheduled: "アンインストール予定",
    uninstalling: "アンインストール中",
    uninstalled: "アンインストール済み",
    // Older hosts exposed Capsule status directly.
    pending: "準備中",
    active: "稼働中",
    stale: "更新あり",
    error: "エラー",
    disabled: "停止中",
    destroyed: "削除済み",
  };
  return Object.hasOwn(labels, state) ? labels[state]! : "状態不明";
}

/** Runs the home lists under 要対応: waiting for approval, or failed. */
export function isAttentionRun(run: Pick<MobileRun, "status">): boolean {
  return run.status === "waiting_approval" || run.status === "failed";
}

/**
 * The home's unread badge: Runs this account can approve now, counted from
 * Takosumi's own data, never from notifications (a notification can be
 * missed, dismissed or never sent; the Run list is what is true).
 */
export function pendingApprovalCount(home: Pick<TakosumiMobileHome, "runs"> | undefined): number {
  return (home?.runs ?? []).filter((run) => canApproveRun(run)).length;
}

export function runStatusLabel(status: MobileRunStatus): string {
  const labels: Record<MobileRunStatus, string> = {
    queued: "待機中",
    running: "実行中",
    waiting_approval: "承認待ち",
    succeeded: "完了",
    failed: "失敗",
    cancelled: "中止",
    expired: "期限切れ",
  };
  return labels[status];
}

export function runTypeLabel(type: MobileRunType): string {
  const labels: Record<MobileRunType, string> = {
    source_sync: "ソース同期",
    compatibility_check: "互換性確認",
    plan: "変更確認",
    apply: "反映",
    destroy_plan: "削除確認",
    destroy_apply: "削除",
    drift_check: "差分確認",
    backup: "バックアップ",
    restore: "復元",
    artifact: "実行準備",
    state_recovery: "状態復旧",
  };
  return labels[type];
}
