import { createSignal, For, Show } from "solid-js";
import {
  CONNECTIONS_KEY, connectionKey, loadWebConnections, newWebConnection, type WebConnection,
} from "./web-connections.ts";

export function WebHome(props: {
  readonly open: (connection: WebConnection) => Promise<void>;
  readonly clearData?: (connection: WebConnection) => Promise<void>;
  readonly storage?: Pick<Storage, "getItem" | "setItem">;
  readonly allowLegacyMobile?: boolean;
}) {
  const [connections, setConnections] = createSignal<readonly WebConnection[]>([]);
  const [url, setUrl] = createSignal("");
  const [name, setName] = createSignal("");
  const [error, setError] = createSignal("");
  const [opening, setOpening] = createSignal<string>();
  const [lastOpened, setLastOpened] = createSignal<string>();
  const [clearCandidate, setClearCandidate] = createSignal<WebConnection>();
  const [clearing, setClearing] = createSignal(false);
  const [status, setStatus] = createSignal("");
  const busy = () => Boolean(opening()) || clearing();
  const locked = () => busy() || clearCandidate() !== undefined;
  try {
    setConnections(loadWebConnections(props.storage));
  } catch {
    setError("接続先の保存内容を読み込めませんでした。");
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
    setOpening(connectionKey(connection));
    try {
      await props.open(connection);
      setLastOpened(connectionKey(connection));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "接続先を開けませんでした。");
    } finally {
      setOpening(undefined);
    }
  }

  function add(event: SubmitEvent) {
    event.preventDefault();
    if (locked()) return;
    setError("");
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
                  onClick={() => { setError(""); save(connections().filter((item) => connectionKey(item) !== connectionKey(connection))); }}>
                  接続先から外す
                </button>
                <Show when={props.clearData && connection.profile !== "shared"}>
                  <button class="web-remove" type="button" disabled={locked()}
                    onClick={() => { setError(""); setStatus(""); setClearCandidate(connection); }}>
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
          <button class="web-primary" type="submit" disabled={locked()}>保存して開く</button>
        </form>
      </section>
      <Show when={error()}><p class="web-error" role="alert">{error()}</p></Show>
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
