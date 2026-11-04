import { createSignal, For, Show } from "solid-js";
import {
  CONNECTIONS_KEY, decodeConnections, webConnection, type WebConnection,
} from "./web-connections.ts";

export function WebHome(props: {
  readonly open: (connection: WebConnection) => Promise<void>;
  readonly storage?: Pick<Storage, "getItem" | "setItem">;
}) {
  const [connections, setConnections] = createSignal<readonly WebConnection[]>([]);
  const [url, setUrl] = createSignal("");
  const [name, setName] = createSignal("");
  const [error, setError] = createSignal("");
  const [opening, setOpening] = createSignal<string>();
  const [lastOpened, setLastOpened] = createSignal<string>();
  try {
    setConnections(decodeConnections(props.storage?.getItem(CONNECTIONS_KEY) ?? null));
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
    if (opening()) return;
    setError("");
    setOpening(connection.origin);
    try {
      await props.open(connection);
      setLastOpened(connection.origin);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "接続先を開けませんでした。");
    } finally {
      setOpening(undefined);
    }
  }

  function add(event: SubmitEvent) {
    event.preventDefault();
    setError("");
    try {
      const connection = webConnection(url(), name());
      const next = connections().filter((item) => item.origin !== connection.origin);
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

  return (
    <main class="web-home">
      <header class="web-heading">
        <span class="web-mark" aria-hidden="true">墨</span>
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
                <button class="web-connection" type="button" disabled={Boolean(opening())}
                  onClick={() => void open(connection)}>
                  <strong>{connection.name}</strong>
                  <span>{connection.origin}</span>
                  <small>{opening() === connection.origin ? "開いています…" :
                    lastOpened() === connection.origin ? "前の画面へ戻る →" : "開く →"}</small>
                </button>
                <button class="web-remove" type="button" disabled={Boolean(opening())}
                  aria-label={`${connection.name}を接続先から外す`}
                  onClick={() => { setError(""); save(connections().filter((item) => item.origin !== connection.origin)); }}>
                  接続先から外す
                </button>
              </li>
            )}</For>
          </ul>
          <p class="web-note">接続先を外してもログアウトはしません。ログアウトは各サイトで行えます。</p>
        </Show>
        <form onSubmit={add}>
          <h3>{connections().length ? "接続先を追加" : "Takosumiにつなぐ"}</h3>
          <label for="connection-url">接続先URL</label>
          <input id="connection-url" type="url" required autocomplete="url"
            placeholder="https://app.takosumi.com" value={url()} disabled={Boolean(opening())}
            onInput={(event) => setUrl(event.currentTarget.value)} />
          <label for="connection-name">名前 <span>（任意）</span></label>
          <input id="connection-name" type="text" maxlength={80} value={name()}
            placeholder="自分のTakosumi" disabled={Boolean(opening())}
            onInput={(event) => setName(event.currentTarget.value)} />
          <button class="web-primary" type="submit" disabled={Boolean(opening())}>保存して開く</button>
        </form>
      </section>
      <Show when={error()}><p class="web-error" role="alert">{error()}</p></Show>
    </main>
  );
}
