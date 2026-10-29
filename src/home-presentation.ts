import type {
  MobileApp,
  MobileWorkspace,
  TakosumiMobileHome,
} from "./api.ts";

export function mobileHomeTitle(_home?: TakosumiMobileHome): string {
  return "Takosumi";
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
  return role === "owner" ? "オーナー" : "メンバー";
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
