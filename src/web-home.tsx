import { createEffect, createSignal, For, onCleanup, Show } from "solid-js";
import {
  CONNECTIONS_KEY, connectionKey, loadWebConnections, newWebConnection, type WebConnection,
} from "./web-connections.ts";
import { BrowserRecoveryRequired } from "./web-profile-recovery.ts";
import type { BootstrapStage } from "./fixture-bootstrap.ts";
import {
  permissionLabels,
  type PendingPermission,
  type PermissionAnswer,
  type WebPermissionPrompts,
} from "./web-permissions.ts";

export function WebHome(props: {
  readonly open: (connection: WebConnection) => Promise<void>;
  readonly clearData?: (connection: WebConnection) => Promise<void>;
  readonly recover?: (connection: WebConnection, ticket: BrowserRecoveryRequired) => Promise<void>;
  readonly permissionPrompts?: WebPermissionPrompts;
  readonly storage?: Pick<Storage, "getItem" | "setItem">;
  readonly allowLegacyMobile?: boolean;
  readonly observeBootstrap?: (stage: BootstrapStage) => void;
}) {
  const observe = (stage: BootstrapStage) => {
    try { props.observeBootstrap?.(stage); } catch { /* Diagnostic only. */ }
  };
  observe("home_init");
  const [connections, setConnections] = createSignal<readonly WebConnection[]>([]);
  const [url, setUrl] = createSignal("");
  const [name, setName] = createSignal("");
  const [error, setError] = createSignal("");
  const [opening, setOpening] = createSignal<string>();
  const [lastOpened, setLastOpened] = createSignal<string>();
  const [clearCandidate, setClearCandidate] = createSignal<WebConnection>();
  const [clearing, setClearing] = createSignal(false);
  const [status, setStatus] = createSignal("");
  const [recovery, setRecovery] = createSignal<{ connection: WebConnection; ticket: BrowserRecoveryRequired }>();
  const [pendingPermissions, setPendingPermissions] = createSignal<readonly PendingPermission[]>([]);
  const [answering, setAnswering] = createSignal<string>();
  let permissionRevision = 0;
  createEffect(() => {
    const prompts = props.permissionPrompts;
    if (!prompts) {
      setPendingPermissions([]);
      return;
    }
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      // Subscribe before the snapshot, but do not let a delayed reply replace
      // an event received since that request. A failed watch still gets a list.
      try {
        const release = await prompts.watch((list) => {
          if (disposed) return;
          permissionRevision += 1;
          setPendingPermissions(list);
        });
        if (disposed) {
          release();
          return;
        }
        unlisten = release;
      } catch { /* Live updates are unavailable; the snapshot still applies. */ }
      try {
        const revision = permissionRevision;
        const list = await prompts.list();
        if (!disposed && revision === permissionRevision) setPendingPermissions(list);
      } catch { /* An unavailable surface shows no prompts. */ }
    })();
    onCleanup(() => {
      disposed = true;
      permissionRevision += 1;
      unlisten?.();
    });
  });
  const busy = () => Boolean(opening()) || clearing();
  const locked = () => busy() || clearCandidate() !== undefined;
  observe("connections_begin");
  try {
    setConnections(loadWebConnections(props.storage));
    observe("connections_end");
  } catch {
    setError("接続先の保存内容を読み込めませんでした。");
    observe("connections_failed");
  }

  function save(next: readonly WebConnection[]): boolean {
    try {
      if (!props.storage) throw new Error();
      props.storage.setItem(CONNECTIONS_KEY, JSON.stringify(next));
      setConnections(next);
      return true;
    } catch {
      setError("接続先を保存できませんでした。端末の空き容量と保存設定を確認してください。");
      return false;
    }
  }

  async function open(connection: WebConnection) {
    if (locked()) return;
    setError("");
    setStatus("");
    setRecovery(undefined);
    setOpening(connectionKey(connection));
    try {
      await props.open(connection);
      setLastOpened(connectionKey(connection));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "接続先を開けませんでした。");
      if (cause instanceof BrowserRecoveryRequired &&
        cause.profile === connection.profile && cause.key === connectionKey(connection)) {
        setRecovery({ connection, ticket: cause });
      }
    } finally {
      setOpening(undefined);
    }
  }

  async function recover() {
    const candidate = recovery();
    const action = props.recover;
    if (!candidate || !action || locked() || !connections().some((item) =>
      connectionKey(item) === connectionKey(candidate.connection))) return;
    setRecovery(undefined);
    setError("");
    setStatus("");
    setOpening(connectionKey(candidate.connection));
    try {
      await action(candidate.connection, candidate.ticket);
      setLastOpened(connectionKey(candidate.connection));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "ブラウザを開き直せませんでした。");
    } finally {
      setOpening(undefined);
    }
  }

  function add(event: SubmitEvent) {
    event.preventDefault();
    if (locked()) return;
    setError("");
    setRecovery(undefined);
    try {
      const connection = newWebConnection(connections(), url(), name());
      const next = connections();
      if (next.length >= 32) throw new Error("接続先は32件まで保存できます。");
      if (save([...next, connection])) {
        setUrl("");
        setName("");
        void open(connection);
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "接続先を確認してください。");
    }
  }

  async function clearData() {
    const target = clearCandidate();
    const clear = props.clearData;
    if (!target || !clear || busy()) return;
    setClearing(true);
    setError("");
    setStatus("");
    setRecovery(undefined);
    try {
      await clear(target);
      if (lastOpened() === connectionKey(target)) setLastOpened(undefined);
      setClearCandidate(undefined);
      setStatus(`${target.name}のサイトデータを消去しました。`);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "保存データを消去できませんでした。");
    } finally {
      setClearing(false);
    }
  }

  async function answer(prompt: PendingPermission, decision: PermissionAnswer) {
    const prompts = props.permissionPrompts;
    if (!prompts || answering() !== undefined) return;
    const revision = ++permissionRevision;
    setAnswering(`${prompt.profile}:${prompt.promptId}`);
    setError("");
    try {
      const list = await prompts.answer(prompt, decision);
      if (revision === permissionRevision) setPendingPermissions(list);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "許可設定を登録できませんでした。");
    } finally {
      setAnswering(undefined);
    }
  }

  function connectionNameOf(prompt: PendingPermission): string {
    return connections().find((item) => item.profile === prompt.profile)?.name ??
      "この接続先";
  }

  function originHost(origin: string): string {
    try {
      return new URL(origin).host;
    } catch {
      return origin;
    }
  }

  return (
    <main class="web-home">
      <header class="web-heading">
        <img class="web-mark" src="/brand/takosumi.png" width="44" height="44" alt="" />
        <h1>Takosumi</h1>
        <p>いつものアプリへ。</p>
      </header>
      <section aria-labelledby="connections-heading">
        <h2 id="connections-heading">接続先</h2>
        <p class="web-description">Takosumiにログインして、ホームからアプリを開きます。</p>
        <Show when={connections().length}>
          <ul class="web-connections">
            <For each={connections()}>{(connection) => (
              <li>
                <button class="web-connection" type="button" disabled={locked()}
                  onClick={() => void open(connection)}>
                  <strong>{connection.name}</strong>
                  <span>{connection.origin}</span>
                  <small>{opening() === connectionKey(connection) ? "開いています…" :
                    lastOpened() === connectionKey(connection) ? "前の画面へ戻る →" : "開く →"}</small>
                </button>
                <button class="web-remove" type="button" disabled={locked()}
                  aria-label={`${connection.name}を接続先から外す`}
                  onClick={() => { setError(""); setRecovery(undefined); save(connections().filter((item) => connectionKey(item) !== connectionKey(connection))); }}>
                  接続先から外す
                </button>
                <Show when={props.clearData && connection.profile !== "shared"}>
                  <button class="web-remove" type="button" disabled={locked()}
                    onClick={() => { setError(""); setStatus(""); setRecovery(undefined); setClearCandidate(connection); }}>
                    保存データを消去…
                  </button>
                </Show>
                <Show when={clearCandidate() && connectionKey(clearCandidate()!) === connectionKey(connection)}>
                  <section class="web-clear" aria-label={`${connection.name}の保存データ消去の確認`}>
                    <h3>{connection.name}の保存データを消去しますか？</h3>
                    <p>この接続先で開いたサイトからログアウトし、Cookie・サイト内の保存内容を消去します。前の画面への復帰も初期化します。接続先の設定は残ります。</p>
                    <p>先にこの接続先の画面をすべて閉じてください。端末へ保存したファイルや、旧モバイル版のログインは対象外です。</p>
                    <button type="button" disabled={busy()} onClick={() => setClearCandidate(undefined)}>キャンセル</button>
                    <button type="button" disabled={busy()} onClick={() => void clearData()}>
                      {clearing() ? "消去しています…" : "保存データを消去"}
                    </button>
                  </section>
                </Show>
              </li>
            )}</For>
          </ul>
          <p class="web-note">接続先を外してもログアウトはしません。ログアウトは各サイトで行えます。</p>
        </Show>
        <form onSubmit={add}>
          <h3>{connections().length ? "接続先を追加" : "Takosumiにつなぐ"}</h3>
          <p class="web-note">同じ接続先を、別のログインで追加することもできます。</p>
          <label for="connection-url">接続先URL</label>
          <input id="connection-url" type="url" required autocomplete="url"
            placeholder="https://app.takosumi.com" value={url()} disabled={locked()}
            onInput={(event) => setUrl(event.currentTarget.value)} />
          <label for="connection-name">名前 <span>（任意）</span></label>
          <input id="connection-name" type="text" maxlength={80} value={name()}
            placeholder="自分のTakosumi" disabled={locked()}
            onInput={(event) => setName(event.currentTarget.value)} />
          <button class="web-primary" type="submit" disabled={locked()}
            ref={() => observe("save_element")}>保存して開く</button>
        </form>
      </section>
      <Show when={pendingPermissions().length}>
        <section aria-labelledby="permissions-heading">
          <h2 id="permissions-heading">アクセスの許可</h2>
          <For each={pendingPermissions()}>{(prompt) => (
            <section class="web-clear"
              aria-label={`${originHost(prompt.origin)}からのアクセスの要求`}>
              <h3>{originHost(prompt.origin)}が{permissionLabels(prompt.permissions, prompt.kind).join("・")}を求めています</h3>
              <p>接続先「{connectionNameOf(prompt)}」で開いているサイトだけに適用されます。</p>
              <button type="button" disabled={answering() !== undefined}
                onClick={() => void answer(prompt, "deny")}>許可しない</button>
              <button type="button" disabled={answering() !== undefined}
                onClick={() => void answer(prompt, "allow")}>
                {answering() === `${prompt.profile}:${prompt.promptId}`
                  ? "登録しています…" : "許可する"}
              </button>
            </section>
          )}</For>
        </section>
      </Show>
      <Show when={error()}><p class="web-error" role="alert">{error()}</p></Show>
      <Show when={props.recover && recovery()}>{(candidate) => (
        <section aria-label="ブラウザの復帰">
          <p class="web-note">{candidate().connection.name}を開き直します。保存データは消去しません。
            直前の未保存の変更は戻らない場合があります。</p>
          <button type="button" disabled={locked()} onClick={() => void recover()}>
            保存データを保持して開き直す
          </button>
          <button type="button" disabled={locked()} onClick={() => setRecovery(undefined)}>今は開き直さない</button>
        </section>
      )}</Show>
      <Show when={status()}><p class="web-note" role="status">{status()}</p></Show>
      <Show when={props.allowLegacyMobile}>
        <details class="web-legacy">
          <summary>以前のモバイル版を使っていた方</summary>
          <p class="web-note">この端末に保存した以前のログインは、旧モバイル画面で利用できます。</p>
          <a href="?web-client=0" onClick={(event) => {
            event.preventDefault();
            location.replace("?web-client=0");
          }}>保存済みログインを開く</a>
        </details>
      </Show>
    </main>
  );
}
