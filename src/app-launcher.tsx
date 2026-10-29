import {
  createEffect,
  createMemo,
  createSignal,
  For,
  on,
  onMount,
  Show,
} from "solid-js";
import {
  mobileErrorMessage,
  type MobilePersistentStore,
  type MobileSession,
} from "@takosjp/mobile-kit";
import { MobilePreviewSection } from "@takosjp/mobile-kit/solid";
import { loadAppScreens } from "./api.ts";
import {
  type AppLauncherEntry,
  type AppUnreadCounts,
  buildLauncherEntries,
  type CapsuleAppScreens,
  type CustomAppEntry,
  type LauncherApp,
  stabilizeLauncherEntries,
  unreadBadgeLabel,
} from "./app-screens.ts";
import type { AppScreenCapabilities, AppScreenOpener } from "./app-screen-native.ts";
import { addCustomApp, readCustomApps, removeCustomApp } from "./custom-apps.ts";
import type { LauncherController } from "./launcher-controller.ts";

/**
 * The home's app grid: each installed app's launchable screens and the URLs
 * the person added, as icons with names. Tapping one opens it in a native
 * app screen. Unread badges read `unreadCounts`, which push delivery
 * (TASK-0119 U4 step 4) fills; it is empty until then.
 */
export function AppLauncher(props: {
  readonly session: MobileSession;
  readonly apps: readonly LauncherApp[];
  readonly store?: MobilePersistentStore;
  readonly controller: LauncherController;
  readonly opener: AppScreenOpener;
  readonly unreadCounts: () => AppUnreadCounts;
}) {
  const [screens, setScreens] = createSignal<ReadonlyMap<string, CapsuleAppScreens>>(
    new Map(),
  );
  const [custom, setCustom] = createSignal<readonly CustomAppEntry[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [editing, setEditing] = createSignal(false);
  const [adding, setAdding] = createSignal(false);
  const [draftUrl, setDraftUrl] = createSignal("");
  const [draftTitle, setDraftTitle] = createSignal("");
  const [status, setStatus] = createSignal<string>();
  const [capabilities, setCapabilities] = createSignal<AppScreenCapabilities>({
    inApp: false,
    pinShortcut: false,
  });

  onMount(() => {
    void readCustomApps(props.store)
      .then(setCustom)
      .catch(() => setCustom([]));
    void props.opener.capabilities().then(setCapabilities);
  });

  // Reload screens only when the set of apps changes, not on every Run poll.
  const appsKey = createMemo(() =>
    props.apps.map((app) => `${app.capsuleId}:${app.state}`).join("\n"),
  );
  let loadGeneration = 0;
  createEffect(
    on(appsKey, () => {
      const generation = ++loadGeneration;
      const apps = props.apps;
      if (apps.length === 0) {
        setScreens(new Map());
        return;
      }
      setLoading(true);
      void loadAppScreens(props.session, apps)
        .then((result) => {
          if (generation !== loadGeneration) return;
          setScreens(result.screens);
          if (result.failedCapsuleIds.length > 0) {
            setStatus("一部のアプリの画面を読み込めませんでした。");
          }
        })
        .catch((error) => {
          if (generation !== loadGeneration) return;
          setStatus(mobileErrorMessage(error, "アプリの画面を読み込めませんでした。"));
        })
        .finally(() => {
          if (generation === loadGeneration) setLoading(false);
        });
    }),
  );

  const entries = createMemo<readonly AppLauncherEntry[]>(
    (previous) =>
      stabilizeLauncherEntries(
        previous,
        buildLauncherEntries({ apps: props.apps, screens: screens(), custom: custom() }),
      ),
    [],
  );
  createEffect(() => props.controller.setEntries(entries()));

  async function open(entry: AppLauncherEntry) {
    setStatus(undefined);
    try {
      await props.controller.openEntry(entry);
    } catch (error) {
      setStatus(mobileErrorMessage(error, `${entry.label} を開けませんでした。`));
    }
  }

  async function pin(entry: AppLauncherEntry) {
    setStatus(undefined);
    try {
      const requested = await props.controller.pinEntry(entry);
      setStatus(
        requested
          ? `${entry.label} をホーム画面に追加する確認を表示しました。`
          : "このホームアプリは、ホーム画面への追加に対応していません。",
      );
    } catch (error) {
      setStatus(mobileErrorMessage(error, "ホーム画面に追加できませんでした。"));
    }
  }

  async function remove(entry: AppLauncherEntry) {
    if (!entry.customId) return;
    setCustom(await removeCustomApp(props.store, entry.customId));
  }

  async function add(event: SubmitEvent) {
    event.preventDefault();
    const result = await addCustomApp({
      store: props.store,
      url: draftUrl(),
      title: draftTitle(),
    });
    if (!result.ok) {
      setStatus(result.error);
      return;
    }
    setCustom(result.entries);
    setDraftUrl("");
    setDraftTitle("");
    setAdding(false);
    setStatus(
      result.existed
        ? `${result.entry.title} はすでに追加されています。`
        : `${result.entry.title} を追加しました。`,
    );
  }

  return (
    <MobilePreviewSection
      title="アプリ"
      class="app-launcher"
      detail={loading() ? "読み込み中" : `${entries().length}件`}
      actions={
        <Show when={entries().some((entry) => entry.source === "custom") || capabilities().pinShortcut}>
          <button
            type="button"
            class="text-button"
            aria-pressed={editing()}
            onClick={() => setEditing(!editing())}
          >
            {editing() ? "完了" : "編集"}
          </button>
        </Show>
      }
    >
      <Show when={status()}>
        {(message) => <p class="mobile-status">{message()}</p>}
      </Show>
      <ul class="launcher-grid">
        <For each={entries()}>
          {(entry) => (
            <LauncherTile
              entry={entry}
              controller={props.controller}
              badge={unreadBadgeLabel(props.unreadCounts(), entry.key)}
              editing={editing()}
              canPin={capabilities().pinShortcut}
              onOpen={() => void open(entry)}
              onPin={() => void pin(entry)}
              onRemove={() => void remove(entry)}
            />
          )}
        </For>
        <li class="launcher-tile">
          <button
            type="button"
            class="launcher-open"
            aria-expanded={adding()}
            onClick={() => setAdding(!adding())}
          >
            <span class="launcher-icon launcher-add-icon" aria-hidden="true">
              +
            </span>
            <span class="launcher-label">URLを追加</span>
          </button>
        </li>
      </ul>
      <Show when={!loading() && entries().length === 0 && !adding()}>
        <p class="empty">
          開ける画面のあるアプリはまだありません。https のURLを追加することもできます。
        </p>
      </Show>
      <Show when={adding()}>
        <form class="launcher-add-form" onSubmit={(event) => void add(event)}>
          <label>
            <span>URL</span>
            <input
              type="url"
              inputmode="url"
              autocomplete="url"
              autocapitalize="off"
              spellcheck={false}
              placeholder="https://example.com"
              value={draftUrl()}
              onInput={(event) => setDraftUrl(event.currentTarget.value)}
              required
            />
          </label>
          <label>
            <span>名前（任意）</span>
            <input
              type="text"
              maxLength={64}
              value={draftTitle()}
              onInput={(event) => setDraftTitle(event.currentTarget.value)}
            />
          </label>
          <p class="empty">この端末の中だけに保存します。https のURLだけ追加できます。</p>
          <div class="run-actions">
            <button type="button" class="text-button" onClick={() => setAdding(false)}>
              やめる
            </button>
            <button type="submit" class="primary">
              追加
            </button>
          </div>
        </form>
      </Show>
    </MobilePreviewSection>
  );
}

function LauncherTile(props: {
  readonly entry: AppLauncherEntry;
  readonly controller: LauncherController;
  readonly badge?: string;
  readonly editing: boolean;
  readonly canPin: boolean;
  readonly onOpen: () => void;
  readonly onPin: () => void;
  readonly onRemove: () => void;
}) {
  const [imageSrc, setImageSrc] = createSignal<string>();
  createEffect(() => {
    const icon = props.entry.icon;
    setImageSrc(undefined);
    void props.controller.iconDataUrl(icon).then((src) => {
      if (props.entry.icon === icon) setImageSrc(src);
    });
  });
  const glyph = () =>
    props.entry.icon?.kind === "glyph" ? props.entry.icon.glyph : props.entry.fallbackGlyph;

  return (
    <li class="launcher-tile">
      <button
        type="button"
        class="launcher-open"
        title={props.entry.host}
        onClick={() => props.onOpen()}
      >
        <span class="launcher-icon" aria-hidden="true">
          <Show when={imageSrc()} fallback={<span class="launcher-glyph">{glyph()}</span>}>
            {(src) => <img src={src()} alt="" draggable={false} />}
          </Show>
          <Show when={props.badge}>
            {(badge) => <span class="launcher-badge">{badge()}</span>}
          </Show>
        </span>
        <span class="launcher-label">{props.entry.label}</span>
        <span class="visually-hidden">
          {props.badge ? `未読 ${props.badge}件、` : ""}
          {props.entry.host}
        </span>
      </button>
      <Show when={props.editing && (props.canPin || props.entry.source === "custom")}>
        <div class="launcher-tile-actions">
          <Show when={props.canPin}>
            <button type="button" class="text-button" onClick={() => props.onPin()}>
              ホームに追加
            </button>
          </Show>
          <Show when={props.entry.source === "custom"}>
            <button type="button" class="text-button danger" onClick={() => props.onRemove()}>
              削除
            </button>
          </Show>
        </div>
      </Show>
    </li>
  );
}
