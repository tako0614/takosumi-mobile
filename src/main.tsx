import { createEffect, createMemo, createSignal, For, Show } from "solid-js";
import {
  formatMobilePreviewDate,
  mobileErrorMessage,
  type MobileSession,
  type NativeBridge,
} from "@takosjp/mobile-kit";
import {
  defineMobileHostActions,
  MobilePreviewCard,
  MobilePreviewList,
  MobilePreviewSection,
  renderMobileClientApp,
  type MobileShellMetric,
} from "@takosjp/mobile-kit/solid";
import {
  approveRun,
  approveRestoreRun,
  canApproveRun,
  canCancelRun,
  cancelRun,
  loadHome,
  loadWorkspaceHome,
  type MobileApp,
  type MobileCapsule,
  type MobileRun,
  type MobileRunStatus,
  type MobileRunType,
  type TakosumiMobileHome,
} from "./api.ts";
import {
  createProductNativeBridge,
  isUnsupportedTauriDesktop,
} from "./native.ts";
import {
  appDisplayName,
  appRoleLabel,
  appStateLabel,
  legacyWorkspaceOptions,
  mobileHomeTitle,
} from "./home-presentation.ts";
import { productAdapter } from "./product.ts";
import { authorizeRunMutation } from "./security.ts";
import "./styles.css";

const metrics = [
  {
    label: "サービス",
    value: (home) => home?.apps.length || home?.capsules.length,
  },
  {
    label: "実行中",
    value: (home) =>
      home?.runs.filter(
        (run) => run.status === "queued" || run.status === "running",
      ).length,
  },
  {
    label: "要対応",
    value: (home) => home?.runs.filter(isAttentionRun).length,
  },
] satisfies readonly MobileShellMetric<TakosumiMobileHome>[];

const actions = defineMobileHostActions<TakosumiMobileHome>([
  { label: "ストア", description: "サービスを追加", path: "/new" },
  { label: "通知", description: "要対応と更新を確認", path: "/notifications" },
  { label: "サービス", description: "すべてのサービス", path: "/services" },
  { label: "設定", description: "アカウント", path: "/settings" },
]);

if (isUnsupportedTauriDesktop()) {
  renderUnsupportedDesktop();
} else {
  startTakosumiMobile();
}

function startTakosumiMobile() {
  const nativeBridge = createProductNativeBridge();
  renderMobileClientApp<TakosumiMobileHome>({
    adapter: productAdapter,
    createNativeBridge: () => nativeBridge,
    loadHome,
    sessionUnlock: {
      restoreMode: "required",
      prompt: {
        title: "Takosumi",
        message: "管理セッションを開きます",
        allowDeviceCredential: true,
        confirmationRequired: true,
      },
    },
    homeLabel: "ホーム",
    copy: {
      eyebrow: "YOUR SERVICES, IN YOUR POCKET",
      // Mirror of takosumi/dashboard/public/tako.png, the canonical Takosumi
      // mark named by docs/reference/design-language.md; `brandMark` stays as
      // the fallback glyph.
      brandLogoUrl: "/brand/takosumi.png",
      brandMark: "T",
      onboardingTitle: "Takosumiを持ち歩こう",
      summary:
        "サービスの確認、通知、承認をスマホから。Cloudにもself-hostにも接続できます。",
      manualActionLabel: "Takosumiに接続",
      manualActionDescription:
        "app.takosumi.com または自分のTakosumi URLを入力",
      connectLabel: "Takosumi URL",
      qrActionLabel: "接続QRを読み取る",
      discoveredHeading: "Takosumiが見つかりました",
      homeFallbackTitle: "Takosumi",
      refreshLabel: "更新",
      homeTitle: mobileHomeTitle,
      metricsLabel: "現在の状態",
      shortcutsLabel: "メニュー",
    },
    metrics,
    hostActions: actions,
    renderHomeExtra: ({ home, session, openHostRoute }) => (
      <TakosumiControlHome
        home={home}
        session={session}
        nativeBridge={nativeBridge}
        openHostRoute={openHostRoute}
      />
    ),
  });
}

function renderUnsupportedDesktop() {
  const root = document.getElementById("root");
  if (!root) throw new Error("Mobile app root not found: root");
  const message = document.createElement("main");
  message.className = "unsupported-platform";
  message.setAttribute("role", "alert");
  message.textContent =
    "Takosumi Mobile は iOS / Android 専用です。このデスクトップ版では認証情報を読み込みません。";
  root.replaceChildren(message);
}

function TakosumiControlHome(props: {
  home?: TakosumiMobileHome;
  session: MobileSession;
  nativeBridge: NativeBridge;
  openHostRoute: (path: string) => Promise<void>;
}) {
  const [current, setCurrent] = createSignal(props.home);
  const [activeSession, setActiveSession] = createSignal(props.session);
  const [loading, setLoading] = createSignal(false);
  const [status, setStatus] = createSignal<string>();

  createEffect(() => setCurrent(props.home));
  createEffect(() => setActiveSession(props.session));

  const attentionRuns = createMemo(() =>
    (current()?.runs ?? []).filter(isAttentionRun),
  );
  const recentRuns = createMemo(() => (current()?.runs ?? []).slice(0, 8));
  const capsuleName = (run: MobileRun) => {
    const app = current()?.apps.find((app) => app.capsuleId === run.capsuleId);
    return app
      ? appDisplayName(app)
      : current()?.capsules.find((capsule) => capsule.id === run.capsuleId)?.name ??
          "Takosumi operation";
  };

  async function reload(workspaceId?: string) {
    setLoading(true);
    setStatus(undefined);
    try {
      const home = current();
      setCurrent(
        home?.workspace
          ? await loadWorkspaceHome(
              activeSession(),
              workspaceId ?? home.workspace.id,
              home.workspaces,
            )
          : await loadHome(activeSession()),
      );
    } catch (error) {
      setStatus(mobileErrorMessage(error, "更新できませんでした。"));
    } finally {
      setLoading(false);
    }
  }

  async function approve(run: MobileRun) {
    setLoading(true);
    try {
      const input = {
        session: activeSession(),
        nativeBridge: props.nativeBridge,
        run,
        authorize: async () =>
          await authorizeRunMutation({
            bridge: props.nativeBridge,
            intent: {
              kind: "approve",
              run,
              capsuleName: capsuleName(run),
            },
          }),
      };
      const result =
        run.type === "restore"
          ? await approveRestoreRun(input)
          : await approveRun(input);
      setActiveSession(result.session);
      setStatus(
        run.type === "restore"
          ? "復元を承認しました。"
          : "変更を承認しました。",
      );
      await reload();
    } catch (error) {
      setStatus(mobileErrorMessage(error, "承認できませんでした。"));
      setLoading(false);
    }
  }

  async function cancel(run: MobileRun) {
    setLoading(true);
    try {
      const result = await cancelRun({
        session: activeSession(),
        nativeBridge: props.nativeBridge,
        run,
        authorize: async () =>
          await authorizeRunMutation({
            bridge: props.nativeBridge,
            intent: {
              kind: "cancel",
              run,
              capsuleName: capsuleName(run),
            },
          }),
      });
      setActiveSession(result.session);
      setStatus("キャンセルを受け付けました。");
      await reload();
    } catch (error) {
      setStatus(mobileErrorMessage(error, "キャンセルできませんでした。"));
      setLoading(false);
    }
  }

  return (
    <div class="takosumi-control-home">
      <Show when={legacyWorkspaceOptions(current()).length > 1}>
        <label class="workspace-picker">
          <span>ワークスペース</span>
          <select
            value={current()?.workspace?.id}
            disabled={loading()}
            onChange={(event) => void reload(event.currentTarget.value)}
          >
            <For each={legacyWorkspaceOptions(current())}>
              {(workspace) => (
                <option value={workspace.id}>{workspace.displayName}</option>
              )}
            </For>
          </select>
        </label>
      </Show>

      <Show when={status()}>
        {(message) => <p class="mobile-status">{message()}</p>}
      </Show>

      <MobilePreviewSection
        title="要対応"
        detail={attentionRuns().length ? `${attentionRuns().length}件` : "なし"}
      >
        <Show
          when={attentionRuns().length}
          fallback={<p class="empty">いま対応が必要な変更はありません。</p>}
        >
          <MobilePreviewList>
            <For each={attentionRuns().slice(0, 5)}>
              {(run) => (
                <li>
                  <MobilePreviewCard class="attention-card">
                    <RunHeading run={run} capsuleName={capsuleName(run)} />
                    <RunSummary run={run} />
                    <div class="run-actions">
                      <button
                        type="button"
                        class="text-button"
                        onClick={() =>
                          void props.openHostRoute(`/runs/${run.id}`)
                        }
                      >
                        詳細
                      </button>
                      <Show when={canApproveRun(run)}>
                        <button
                          type="button"
                          class="primary"
                          disabled={loading()}
                          onClick={() => void approve(run)}
                        >
                          {run.type === "restore"
                            ? "復元を承認"
                            : "承認する"}
                        </button>
                      </Show>
                    </div>
                  </MobilePreviewCard>
                </li>
              )}
            </For>
          </MobilePreviewList>
        </Show>
      </MobilePreviewSection>

      <MobilePreviewSection
        title="サービス"
        detail={`${current()?.apps.length || current()?.capsules.length || 0}件`}
      >
        <Show
          when={current()?.apps.length}
          fallback={
            <Show
              when={current()?.capsules.length}
              fallback={
                <p class="empty">
                  サービスはまだありません。ストアから追加できます。
                </p>
              }
            >
              <MobilePreviewList>
                <For each={current()?.capsules.slice(0, 6)}>
                  {(capsule) => (
                    <li>
                      <button
                        type="button"
                        class="capsule-row"
                        onClick={() =>
                          void props.openHostRoute(`/services/${capsule.id}`)
                        }
                      >
                        <CapsuleMark capsule={capsule} />
                        <span>
                          <strong>{capsule.name}</strong>
                          <small>{capsule.environment}</small>
                        </span>
                        <em data-status={capsule.status}>
                          {capsuleStatusLabel(capsule)}
                        </em>
                      </button>
                    </li>
                  )}
                </For>
              </MobilePreviewList>
            </Show>
          }
        >
          <MobilePreviewList>
            <For each={current()?.apps.slice(0, 6)}>
              {(app) => (
                <li>
                  <button
                    type="button"
                    class="capsule-row"
                    onClick={() =>
                      void props.openHostRoute(`/services/${app.capsuleId}`)
                    }
                  >
                    <AppMark app={app} />
                    <span>
                      <strong>{appDisplayName(app)}</strong>
                      <small>
                        {appRoleLabel(app.role)} · {app.ownerDisplayName}
                      </small>
                    </span>
                    <em data-status={app.state}>{appStateLabel(app.state)}</em>
                  </button>
                </li>
              )}
            </For>
          </MobilePreviewList>
        </Show>
      </MobilePreviewSection>

      <MobilePreviewSection
        title="最近の実行"
        detail={`${recentRuns().length}件`}
      >
        <MobilePreviewList>
          <For each={recentRuns()}>
            {(run) => (
              <li>
                <MobilePreviewCard class="run-card">
                  <RunHeading run={run} capsuleName={capsuleName(run)} />
                  <footer>
                    <time>
                      {formatMobilePreviewDate(run.createdAt, "ja-JP")}
                    </time>
                    <div class="run-actions">
                      <Show when={canCancelRun(run)}>
                        <button
                          type="button"
                          class="text-button danger"
                          disabled={loading()}
                          onClick={() => void cancel(run)}
                        >
                          中止
                        </button>
                      </Show>
                      <button
                        type="button"
                        class="text-button"
                        onClick={() =>
                          void props.openHostRoute(`/runs/${run.id}`)
                        }
                      >
                        開く
                      </button>
                    </div>
                  </footer>
                </MobilePreviewCard>
              </li>
            )}
          </For>
        </MobilePreviewList>
      </MobilePreviewSection>
    </div>
  );
}

function AppMark(props: { app: MobileApp }) {
  return (
    <span class="capsule-mark" aria-hidden="true">
      {appDisplayName(props.app).slice(0, 1).toUpperCase()}
    </span>
  );
}

function RunHeading(props: { run: MobileRun; capsuleName: string }) {
  return (
    <header class="run-heading">
      <span>
        <strong>{props.capsuleName}</strong>
        <small>{runTypeLabel(props.run.type)}</small>
      </span>
      <em data-status={props.run.status}>{runStatusLabel(props.run.status)}</em>
    </header>
  );
}

function RunSummary(props: { run: MobileRun }) {
  const summary = () => props.run.summary;
  return (
    <Show when={summary()}>
      {(value) => (
        <p class="run-summary">
          追加 {value().add ?? 0} ・変更 {value().change ?? 0} ・削除{" "}
          {value().destroy ?? 0}
        </p>
      )}
    </Show>
  );
}

function CapsuleMark(props: { capsule: MobileCapsule }) {
  return (
    <span class="capsule-mark">
      {props.capsule.name.slice(0, 1).toUpperCase()}
    </span>
  );
}

function isAttentionRun(run: MobileRun): boolean {
  return run.status === "waiting_approval" || run.status === "failed";
}

function capsuleStatusLabel(capsule: MobileCapsule): string {
  const labels: Record<MobileCapsule["status"], string> = {
    pending: "準備中",
    active: "稼働中",
    stale: "更新あり",
    error: "エラー",
    disabled: "停止中",
    uninstalled: "未インストール",
    destroyed: "削除済み",
  };
  return labels[capsule.status];
}

function runStatusLabel(status: MobileRunStatus): string {
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

function runTypeLabel(type: MobileRunType): string {
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
