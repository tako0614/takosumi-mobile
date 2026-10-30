import { expect, test } from "bun:test";
import type { MobileApp, MobileRun, MobileWorkspace, TakosumiMobileHome } from "../src/api.ts";
import {
  appDisplayName,
  appRoleLabel,
  appStateLabel,
  isAttentionRun,
  legacyWorkspaceOptions,
  mobileHomeTitle,
  pendingApprovalCount,
} from "../src/home-presentation.ts";

const app: MobileApp = {
  capsuleId: "cap_office",
  name: "office",
  displayName: { ja: "オフィス", en: "Office" },
  role: "owner",
  ownerDisplayName: "Owner account",
  state: "installed",
  primaryAction: "open",
};
const home: TakosumiMobileHome = {
  apps: [app],
  workspaces: [],
  capsules: [],
  runs: [],
};

test("Home retains the Takosumi title regardless of app order or legacy context", () => {
  expect(mobileHomeTitle()).toBe("Takosumi");
  expect(mobileHomeTitle(home)).toBe("Takosumi");
  expect(mobileHomeTitle({ ...home, apps: [{ ...app, displayName: "Another app" }] })).toBe("Takosumi");
  expect(mobileHomeTitle({ ...home, apps: [] })).toBe("Takosumi");
});

test("app names follow locale and retain string and canonical-name fallbacks", () => {
  expect(appDisplayName(app, "ja-JP")).toBe("オフィス");
  expect(appDisplayName(app, "ja")).toBe("オフィス");
  expect(appDisplayName(app, "en-US")).toBe("Office");
  expect(appDisplayName(app, "en-GB")).toBe("Office");
  expect(appDisplayName(app, "fr-FR")).toBe("Office");
  expect(appDisplayName({ ...app, displayName: "Custom label" }, "ja-JP")).toBe("Custom label");
  expect(appDisplayName({ ...app, displayName: undefined }, "en-US")).toBe("office");
});

test("the Japanese home labels every app lifecycle state and membership role", () => {
  const expected = {
    installing: "インストール中",
    installed: "稼働中",
    updating: "更新中",
    needs_attention: "要対応",
    stopped: "停止中",
    uninstall_scheduled: "アンインストール予定",
    uninstalling: "アンインストール中",
    uninstalled: "アンインストール済み",
  };
  for (const [state, label] of Object.entries(expected)) {
    expect(appStateLabel(state)).toBe(label);
  }
  expect(appStateLabel("active")).toBe("稼働中");
  expect(appStateLabel("future_state")).toBe("状態不明");
  expect(appStateLabel("__proto__")).toBe("状態不明");
  expect(appRoleLabel("owner")).toBe("所有者");
  expect(appRoleLabel("member")).toBe("メンバー");
});

test("Workspace controls are absent from app-scoped and empty app homes", () => {
  expect(legacyWorkspaceOptions(undefined)).toEqual([]);
  expect(legacyWorkspaceOptions(home)).toEqual([]);
  expect(legacyWorkspaceOptions({ ...home, apps: [] })).toEqual([]);

  const workspace: MobileWorkspace = {
    id: "ws_legacy",
    handle: "legacy",
    displayName: "Legacy",
    type: "personal",
    ownerUserId: "account_owner",
    createdAt: "2026-10-28T00:00:00.000Z",
    updatedAt: "2026-10-28T00:00:00.000Z",
  };
  expect(legacyWorkspaceOptions({
    ...home,
    apps: [],
    workspace,
    workspaces: [workspace],
  })).toEqual([workspace]);
});

function run(input: Partial<MobileRun>): MobileRun {
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

test("the home's unread badge counts Runs this account can approve, from Takosumi's own data", () => {
  expect(pendingApprovalCount(undefined)).toBe(0);
  expect(
    pendingApprovalCount({
      runs: [
        run({ id: "1" }),
        run({ id: "2", type: "destroy_plan" }),
        run({ id: "3", type: "restore" }),
        // Not approvable: an apply waiting, a failed plan, a finished plan.
        run({ id: "4", type: "apply" }),
        run({ id: "5", status: "failed" }),
        run({ id: "6", status: "succeeded" }),
      ],
    }),
  ).toBe(3);
  expect(isAttentionRun(run({ status: "failed" }))).toBe(true);
  expect(isAttentionRun(run({ status: "running" }))).toBe(false);
});
