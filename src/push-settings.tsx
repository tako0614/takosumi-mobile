import { createEffect, createSignal, on, Show } from "solid-js";
import { mobileAccountIdentity, mobileErrorMessage, type MobileSession } from "@takosjp/mobile-kit";
import { MobilePreviewSection } from "@takosjp/mobile-kit/solid";
import {
  PushError,
  pushProblemMessage,
  type PushCheckResult,
  type PushManager,
} from "./push-manager.ts";

/** What a check found, in words; nothing when all is well. */
export function pushCheckMessage(result: PushCheckResult): string | undefined {
  switch (result.kind) {
    case "removed":
      return "この端末の通知は、Takosumiのアカウント設定から外されたためオフになりました。";
    case "resubscribed":
      return "このTakosumiの通知の鍵が変わったため、通知を登録し直しました。";
    case "unavailable":
      return pushProblemMessage(result.problem);
    default:
      return undefined;
  }
}

/**
 * Notifications for the current Takosumi (instance and account): the switch,
 * the one-time question after sign-in, and what went wrong in plain words.
 * Each Takosumi has its own switch; the others keep theirs.
 */
export function PushSettings(props: {
  readonly session: MobileSession;
  readonly manager: PushManager;
}) {
  const [enabled, setEnabled] = createSignal(false);
  const [prompt, setPrompt] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [status, setStatus] = createSignal<string>();
  const problem = () => props.manager.availability();

  // Once per instance and account shown (a refreshed token is the same one).
  createEffect(
    on(
      () => mobileAccountIdentity(props.session),
      (identity) => {
        setStatus(undefined);
        const session = props.session;
        void (async () => {
          const result = await props.manager.check(session).catch((): PushCheckResult => ({ kind: "unknown" }));
          if (identity !== mobileAccountIdentity(props.session)) return;
          setEnabled(await props.manager.isEnabled(session));
          setStatus(pushCheckMessage(result));
          setPrompt(await props.manager.shouldPrompt(session));
        })();
      },
    ),
  );

  async function enable() {
    setBusy(true);
    setStatus(undefined);
    try {
      await props.manager.enable(props.session);
      setEnabled(true);
      setPrompt(false);
      setStatus("このTakosumiの通知をオンにしました。");
    } catch (error) {
      setStatus(error instanceof PushError ? error.message : mobileErrorMessage(error, "通知をオンにできませんでした。"));
    } finally {
      setBusy(false);
    }
  }

  async function disable() {
    setBusy(true);
    setStatus(undefined);
    try {
      await props.manager.disable(props.session);
      setEnabled(false);
      setStatus("このTakosumiの通知をオフにしました。");
    } catch (error) {
      setStatus(mobileErrorMessage(error, "通知をオフにできませんでした。"));
    } finally {
      setBusy(false);
    }
  }

  async function decline() {
    setPrompt(false);
    await props.manager.markPrompted(props.session).catch(() => undefined);
  }

  return (
    <MobilePreviewSection title="通知" detail={enabled() ? "オン" : "オフ"}>
      <div class="push-settings">
        <Show when={prompt() && !enabled() && !problem()}>
          <div class="push-prompt" role="group" aria-label="通知の確認">
            <p>承認待ちや失敗など、このTakosumiからのお知らせを通知で受け取りますか？</p>
            <div class="run-actions">
              <button type="button" class="text-button" disabled={busy()} onClick={() => void decline()}>
                今はしない
              </button>
              <button type="button" class="primary" disabled={busy()} onClick={() => void enable()}>
                受け取る
              </button>
            </div>
          </div>
        </Show>
        <Show
          when={!problem()}
          fallback={<p class="empty">{pushProblemMessage(problem()!)}</p>}
        >
          <div class="push-switch">
            <span>
              <strong>このTakosumiの通知</strong>
              <small>承認待ち、失敗、インストールの完了など</small>
            </span>
            <button
              type="button"
              class={enabled() ? "text-button" : "primary"}
              role="switch"
              aria-checked={enabled()}
              disabled={busy()}
              onClick={() => void (enabled() ? disable() : enable())}
            >
              {enabled() ? "オフにする" : "オンにする"}
            </button>
          </div>
        </Show>
        <Show when={status()}>{(message) => <p class="mobile-status">{message()}</p>}</Show>
      </div>
    </MobilePreviewSection>
  );
}
