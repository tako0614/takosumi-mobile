import { createMemo, createSignal, Show } from "solid-js";
import { mobileErrorMessage } from "@takosjp/mobile-kit";
import {
  clientForPlatform,
  clientLabel,
  clientStoreHost,
  clientTargetLabel,
} from "./app-clients.ts";
import type { ChooserRequest, OpenMode, OpenRouter } from "./open-routing.ts";
import { displayHost } from "./url-policy.ts";

/**
 * The chooser (選ぶ画面): how to open one app. Same order as the dashboard's
 * 「スマホで使う」 (`AppPhoneSection`): the declared native app, the web, the
 * browser view, inside Takosumi, sharing the URL, and remembering the choice.
 *
 * An unverified native app is shown with its checked name or 「未確認のアプリ」,
 * its real destination, the host it is given and where its install link
 * goes, and opens only after a second, explicit confirmation. Its choice is
 * never remembered.
 */
export function OpenChooser(props: {
  readonly request: ChooserRequest;
  readonly router: OpenRouter;
  readonly platform: string;
  readonly openWeb: (url: string) => Promise<void>;
  readonly copyText?: (text: string) => Promise<void>;
  readonly onClose: () => void;
}) {
  const target = () => props.request.target;
  const client = createMemo(() => clientForPlatform(target().clients, props.platform));
  const host = () => displayHost(target().url);
  // A missing native app keeps its remembered choice unless the person
  // explicitly remembers another way (it is used again once installed).
  const [remember, setRemember] = createSignal(
    props.request.remembered !== undefined && props.request.reason !== "not-installed",
  );
  const [remembered, setRemembered] = createSignal(props.request.remembered);
  // An unverified app always takes two taps: 「…で開く」, then 「確認して開く」.
  const [confirming, setConfirming] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [status, setStatus] = createSignal<string>();
  const [showHomeSteps, setShowHomeSteps] = createSignal(false);

  async function pick(mode: OpenMode, confirmedUnverified = false) {
    if (busy()) return;
    setBusy(true);
    setStatus(undefined);
    try {
      const outcome = await props.router.openWith(target(), mode, {
        remember: remember(),
        confirmedUnverified,
      });
      if (outcome.kind === "needs-confirmation") {
        setConfirming(true);
        return;
      }
      // not-installed reopens the chooser with that reason.
      if (outcome.kind === "opened") props.onClose();
    } catch (error) {
      setStatus(mobileErrorMessage(error, `${target().label} を開けませんでした。`));
    } finally {
      setBusy(false);
    }
  }

  async function installClient() {
    const url = client()?.storeUrl;
    if (!url) return;
    try {
      await props.openWeb(url);
    } catch (error) {
      setStatus(mobileErrorMessage(error, "ストアを開けませんでした。"));
    }
  }

  async function share() {
    const url = target().url;
    try {
      if (typeof navigator !== "undefined" && typeof navigator.share === "function") {
        await navigator.share({ title: target().label, url });
        return;
      }
      if (props.copyText) {
        await props.copyText(url);
        setStatus("URLをコピーしました。");
      }
    } catch (error) {
      if (error instanceof DOMException && error.name === "AbortError") return;
      setStatus(mobileErrorMessage(error, "共有できませんでした。"));
    }
  }

  async function forget() {
    await props.router.forget(target());
    setRemembered(undefined);
    setRemember(false);
    setStatus("覚えた開き方を消しました。");
  }

  const notInstalled = () => props.request.reason === "not-installed";

  return (
    <div class="open-chooser-backdrop" role="presentation" onClick={() => props.onClose()}>
      <section
        class="open-chooser"
        role="dialog"
        aria-modal="true"
        aria-labelledby="open-chooser-title"
        onClick={(event) => event.stopPropagation()}
      >
        <header class="open-chooser-header">
          <h2 id="open-chooser-title">{target().label} の開き方</h2>
          <p class="open-chooser-host">接続先: {host()}</p>
        </header>

        <Show when={status()}>{(message) => <p class="mobile-status">{message()}</p>}</Show>

        <Show when={client()}>
          {(declared) => (
            <div class="open-option" data-option="native">
              <h3>
                専用アプリ: {clientLabel(declared())}
                <Show when={!declared().verified}>
                  <span class="open-option-flag">未確認</span>
                </Show>
              </h3>
              <Show when={notInstalled()}>
                <p class="open-option-note">
                  {clientLabel(declared())} が見つかりませんでした。入れるか、ほかの開き方を選んでください。
                </p>
              </Show>
              <Show when={!declared().verified}>
                <dl class="open-option-facts">
                  <dt>開くアプリ</dt>
                  <dd>{clientTargetLabel(declared())}</dd>
                  <dt>渡す接続先</dt>
                  <dd>{host()}</dd>
                  <Show when={clientStoreHost(declared())}>
                    {(storeHost) => (
                      <>
                        <dt>入れる先</dt>
                        <dd>{storeHost()}</dd>
                      </>
                    )}
                  </Show>
                </dl>
              </Show>
              <Show
                when={!declared().verified && confirming()}
                fallback={
                  <button
                    type="button"
                    class={declared().verified ? "primary" : "text-button"}
                    disabled={busy()}
                    onClick={() => void pick("native")}
                  >
                    {clientLabel(declared())}で開く
                  </button>
                }
              >
                <div class="open-confirm" role="group" aria-label="未確認のアプリを開く確認">
                  <p>
                    このアプリはこのTakosumiで確認されていません。{clientTargetLabel(declared())}
                    を開き、接続先 {host()} を渡します。開きますか？
                  </p>
                  <div class="run-actions">
                    <button type="button" class="text-button" onClick={() => setConfirming(false)}>
                      やめる
                    </button>
                    <button
                      type="button"
                      class="primary"
                      disabled={busy()}
                      onClick={() => void pick("native", true)}
                    >
                      確認して開く
                    </button>
                  </div>
                </div>
              </Show>
              <Show when={declared().storeUrl}>
                <button type="button" class="text-button" onClick={() => void installClient()}>
                  {clientLabel(declared())}を入れる
                </button>
              </Show>
              <p class="open-option-push">通知: 専用アプリが受け取ります。</p>
              <Show when={!declared().verified && remember()}>
                <p class="open-option-note">未確認のアプリで開く選択は覚えません。</p>
              </Show>
            </div>
          )}
        </Show>

        <div class="open-option" data-option="web">
          <h3>web</h3>
          <button type="button" class="text-button" disabled={busy()} onClick={() => void pick("web")}>
            ブラウザで開く
          </button>
          <button
            type="button"
            class="text-button"
            aria-expanded={showHomeSteps()}
            onClick={() => setShowHomeSteps(!showHomeSteps())}
          >
            ホーム画面に追加する
          </button>
          <Show when={showHomeSteps()}>
            <p class="open-option-note">
              {props.platform === "ios"
                ? "Safariで開いてから、共有ボタン（見当たらないときは「…」の中）→「ホーム画面に追加」。"
                : "Chromeで開いてから、右上のメニュー（⋮）→「ホーム画面に追加」または「アプリをインストール」。"}
            </p>
          </Show>
          <p class="open-option-push">
            通知: アプリが対応していれば届きます。iPhoneはホーム画面に追加したときだけ。
          </p>
        </div>

        <div class="open-option" data-option="browser-view">
          <h3>ブラウザの画面</h3>
          <button
            type="button"
            class="text-button"
            disabled={busy()}
            onClick={() => void pick("browser-view")}
          >
            ブラウザの画面で開く
          </button>
          <p class="open-option-push">
            {props.platform === "ios"
              ? "通知: 届きません。"
              : "通知: アプリが対応していれば、ブラウザの通知として届きます。"}
          </p>
        </div>

        <div class="open-option" data-option="in-app">
          <h3>Takosumiの中</h3>
          <button
            type="button"
            class={client()?.verified ? "text-button" : "primary"}
            disabled={busy()}
            onClick={() => void pick("in-app")}
          >
            Takosumiの中で見る
          </button>
          <p class="open-option-push">通知: 届きません。</p>
        </div>

        <div class="open-option" data-option="share">
          <button type="button" class="text-button" onClick={() => void share()}>
            URLを共有
          </button>
          <p class="open-option-note">使うだけの人には、このURLを渡せば足ります。</p>
        </div>

        <label class="open-remember">
          <input
            type="checkbox"
            checked={remember()}
            onChange={(event) => setRemember(event.currentTarget.checked)}
          />
          <span>この端末で覚える</span>
        </label>
        <Show when={remembered()}>
          {(mode) => (
            <p class="open-option-note">
              いまの開き方: {openModeLabel(mode())}{" "}
              <button type="button" class="text-button" onClick={() => void forget()}>
                覚えた開き方を消す
              </button>
            </p>
          )}
        </Show>

        <div class="run-actions">
          <button type="button" class="text-button" onClick={() => props.onClose()}>
            閉じる
          </button>
        </div>
      </section>
    </div>
  );
}

export function openModeLabel(mode: OpenMode): string {
  switch (mode) {
    case "native":
      return "専用アプリ";
    case "web":
      return "ブラウザ";
    case "browser-view":
      return "ブラウザの画面";
    case "in-app":
      return "Takosumiの中";
  }
}
