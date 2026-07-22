import { createEffect, createMemo, createSignal, For, Show } from "solid-js";
import {
  confirmMobileAction,
  formatMobilePreviewDate,
  mobileErrorMessage,
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
  cancelRun,
  loadHome,
  loadWorkspaceHome,
  type MobileCapsule,
  type MobileRun,
  type MobileRunStatus,
  type MobileRunType,
  type TakosumiMobileHome,
} from "./api.ts";
import { createProductNativeBridge } from "./native.ts";
import { productAdapter } from "./product.ts";
import "./styles.css";

const metrics = [
  { label: "サービス", value: (home) => home?.capsules.length },
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
  { label: "設定", description: "Workspaceとアカウント", path: "/settings" },
]);

renderMobileClientApp<TakosumiMobileHome>({
  adapter: productAdapter,
  createNativeBridge: createProductNativeBridge,
  loadHome,
  sessionUnlock: {
    restoreMode: "if-available",
    prompt: {
      title: "Takosumi",
      message: "管理セッションを開きます",
      allowDeviceCredential: true,
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
    manualActionDescription: "app.takosumi.com または自分のTakosumi URLを入力",
    connectLabel: "Takosumi URL",
    qrActionLabel: "接続QRを読み取る",
    discoveredHeading: "Takosumiが見つかりました",
    homeFallbackTitle: "Takosumi",
    refreshLabel: "更新",
    homeTitle: (home) => home?.workspace?.displayName,
    metricsLabel: "現在の状態",
    shortcutsLabel: "メニュー",
  },
  metrics,
  hostActions: actions,
  renderHomeExtra: ({ home, session, openHostRoute }) => (
    <TakosumiControlHome
      home={home}
      session={session}
      openHostRoute={openHostRoute}
    />
  ),
});

function TakosumiControlHome(props: {
  home?: TakosumiMobileHome;
  session: Parameters<typeof loadHome>[0];
  openHostRoute: (path: string) => Promise<void>;
}) {
  const [current, setCurrent] = createSignal(props.home);
  const [loading, setLoading] = createSignal(false);
  const [status, setStatus] = createSignal<string>();

  createEffect(() => setCurrent(props.home));

  const attentionRuns = createMemo(() =>
    (current()?.runs ?? []).filter(isAttentionRun),
  );
  const recentRuns = createMemo(() => (current()?.runs ?? []).slice(0, 8));
  const capsuleName = (run: MobileRun) =>
    current()?.capsules.find((capsule) => capsule.id === run.capsuleId)?.name ??
    "Workspace operation";

  async function reload(workspaceId?: string) {
    const activeId = workspaceId ?? current()?.workspace?.id;
    if (!activeId) return;
    setLoading(true);
    setStatus(undefined);
    try {
      setCurrent(
        await loadWorkspaceHome(
          props.session,
          activeId,
          current()?.workspaces ?? props.home?.workspaces,
        ),
      );
    } catch (error) {
      setStatus(mobileErrorMessage(error, "更新できませんでした。"));
    } finally {
      setLoading(false);
    }
  }

  async function approve(run: MobileRun) {
    if (
      !confirmMobileAction({
        message: `${capsuleName(run)} の変更を承認しますか？`,
      })
    )
      return;
    setLoading(true);
    try {
      await approveRun(props.session, run.id);
      setStatus("変更を承認しました。");
      await reload();
    } catch (error) {
      setStatus(mobileErrorMessage(error, "承認できませんでした。"));
      setLoading(false);
    }
  }

  async function cancel(run: MobileRun) {
    if (!confirmMobileAction({ message: "この実行をキャンセルしますか？" }))
      return;
    setLoading(true);
    try {
      await cancelRun(props.session, run.id);
      setStatus("キャンセルを受け付けました。");
      await reload();
    } catch (error) {
      setStatus(mobileErrorMessage(error, "キャンセルできませんでした。"));
      setLoading(false);
    }
  }

  return (
    <div class="takosumi-control-home">
      <section class="workspace-picker" aria-label="Workspace">
        <div>
          <span>Workspace</span>
          <strong>
            {current()?.workspace?.displayName ?? "Workspaceなし"}
          </strong>
        </div>
        <Show when={(current()?.workspaces.length ?? 0) > 1}>
          <select
            value={current()?.workspace?.id}
            disabled={loading()}
            onChange={(event) => void reload(event.currentTarget.value)}
          >
            <For each={current()?.workspaces}>
              {(workspace) => (
                <option value={workspace.id}>{workspace.displayName}</option>
              )}
            </For>
          </select>
        </Show>
      </section>

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
                      <Show when={run.status === "waiting_approval"}>
                        <button
                          type="button"
                          class="primary"
                          disabled={loading()}
                          onClick={() => void approve(run)}
                        >
                          承認する
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
        detail={`${current()?.capsules.length ?? 0}件`}
      >
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
                      <Show
                        when={
                          run.status === "queued" || run.status === "running"
                        }
                      >
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
  return {
    pending: "準備中",
    active: "稼働中",
    stale: "更新あり",
    error: "エラー",
    disabled: "停止中",
    destroyed: "削除済み",
  }[capsule.status];
}

function runStatusLabel(status: MobileRunStatus): string {
  return {
    queued: "待機中",
    running: "実行中",
    waiting_approval: "承認待ち",
    succeeded: "完了",
    failed: "失敗",
    cancelled: "中止",
    expired: "期限切れ",
  }[status];
}

function runTypeLabel(type: MobileRunType): string {
  return {
    source_sync: "ソース同期",
    compatibility_check: "互換性確認",
    plan: "変更確認",
    apply: "反映",
    destroy_plan: "削除確認",
    destroy_apply: "削除",
    drift_check: "差分確認",
    backup: "バックアップ",
    restore: "復元",
  }[type];
}
