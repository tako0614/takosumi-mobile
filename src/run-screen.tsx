import { createResource, createSignal, Show } from "solid-js";
import { formatMobilePreviewDate, mobileErrorMessage } from "@takosjp/mobile-kit";
import { canApproveRun, canCancelRun, type MobileRun } from "./api.ts";
import { runStatusLabel, runTypeLabel } from "./home-presentation.ts";

/**
 * The Run screen a notification (`/runs/{id}`) opens inside Takosumi Mobile:
 * what the Run is, its state, and approval or cancellation when it can take
 * them. Everything else about the Run is on the dashboard, one tap away.
 */
export function RunScreen(props: {
  readonly runId: string;
  readonly host: string;
  readonly loadRun: (runId: string) => Promise<MobileRun>;
  readonly appName: (run: MobileRun) => string;
  readonly approve: (run: MobileRun) => Promise<boolean>;
  readonly cancel: (run: MobileRun) => Promise<boolean>;
  readonly openDashboard: () => void;
  readonly onClose: () => void;
}) {
  const [run, { refetch }] = createResource(() => props.runId, (runId) => props.loadRun(runId));
  const [busy, setBusy] = createSignal(false);
  const [status, setStatus] = createSignal<string>();

  async function act(action: (run: MobileRun) => Promise<boolean>, current: MobileRun) {
    setBusy(true);
    setStatus(undefined);
    try {
      if (await action(current)) await refetch();
    } catch (error) {
      setStatus(mobileErrorMessage(error, "操作できませんでした。"));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="open-chooser-backdrop" role="presentation" onClick={() => props.onClose()}>
      <section
        class="open-chooser run-sheet"
        role="dialog"
        aria-modal="true"
        aria-labelledby="run-sheet-title"
        onClick={(event) => event.stopPropagation()}
      >
        <header class="open-chooser-header">
          <h2 id="run-sheet-title">{run() ? props.appName(run()!) : "実行"}</h2>
          <p class="open-chooser-host">接続先: {props.host}</p>
        </header>

        <Show when={status()}>{(message) => <p class="mobile-status">{message()}</p>}</Show>

        <Show
          when={run()}
          fallback={
            <Show
              when={run.error}
              fallback={<p class="empty">読み込み中…</p>}
            >
              <p class="mobile-status">
                {mobileErrorMessage(run.error, "この実行を読み込めませんでした。")}
              </p>
            </Show>
          }
        >
          {(current) => (
            <>
              <dl class="open-option-facts run-sheet-facts">
                <dt>種類</dt>
                <dd>{runTypeLabel(current().type)}</dd>
                <dt>状態</dt>
                <dd data-status={current().status}>{runStatusLabel(current().status)}</dd>
                <dt>作成</dt>
                <dd>{formatMobilePreviewDate(current().createdAt, "ja-JP")}</dd>
                <Show when={current().summary}>
                  {(summary) => (
                    <>
                      <dt>内容</dt>
                      <dd>
                        追加 {summary().add ?? 0} ・変更 {summary().change ?? 0} ・削除 {summary().destroy ?? 0}
                      </dd>
                    </>
                  )}
                </Show>
              </dl>
              <div class="run-actions">
                <Show when={canCancelRun(current())}>
                  <button
                    type="button"
                    class="text-button danger"
                    disabled={busy()}
                    onClick={() => void act(props.cancel, current())}
                  >
                    中止
                  </button>
                </Show>
                <Show when={canApproveRun(current())}>
                  <button
                    type="button"
                    class="primary"
                    disabled={busy()}
                    onClick={() => void act(props.approve, current())}
                  >
                    {current().type === "restore" ? "復元を承認" : "承認する"}
                  </button>
                </Show>
              </div>
            </>
          )}
        </Show>

        <div class="run-actions">
          <button type="button" class="text-button" onClick={() => props.openDashboard()}>
            ダッシュボードで開く
          </button>
          <button type="button" class="text-button" onClick={() => props.onClose()}>
            閉じる
          </button>
        </div>
      </section>
    </div>
  );
}
