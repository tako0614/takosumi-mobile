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
  buildLauncherEntries,
  type CapsuleAppScreens,
  type CustomAppEntry,
  type LauncherApp,
  stabilizeLauncherEntries,
} from "./app-screens.ts";
import type {
  AppScreenAction,
  AppScreenCapabilities,
  AppScreenOpener,
} from "./app-screen-native.ts";
import { addCustomApp, readCustomApps, removeCustomApp } from "./custom-apps.ts";
import type { LauncherController } from "./launcher-controller.ts";
import { OpenChooser } from "./open-chooser.tsx";
import {
  type ChooserRequest,
  type OpenContext,
  type OpenRouter,
  type RecentApp,
  recentLauncherEntries,
} from "./open-routing.ts";

/**
 * The home's app grid: each installed app's launchable screens and the URLs
 * the person added, as icons with names, with the recently used apps above
 * them. Tapping one opens it the way the open router decides (the remembered
 * choice, the app's verified native app, or inside Takosumi), and the
 * chooser lets the person pick another way.
 */
export function AppLauncher(props: {
  readonly session: MobileSession;
  readonly apps: readonly LauncherApp[];
  readonly store?: MobilePersistentStore;
  readonly controller: LauncherController;
  readonly opener: AppScreenOpener;
  readonly router: OpenRouter;
  readonly platform: string;
  readonly context?: OpenContext;
  readonly chooser: () => ChooserRequest | undefined;
  readonly closeChooser: () => void;
  readonly recent: () => readonly RecentApp[];
  readonly setRecent: (recent: readonly RecentApp[]) => void;
  /** A bar action waiting for the app list (開き方を変える from an app screen). */
  readonly pendingAction: () => AppScreenAction | undefined;
  readonly clearPendingAction: () => void;
  readonly openStore: (url: string) => Promise<void>;
  readonly copyText?: (text: string) => Promise<void>;
}) {
  const [screens, setScreens] = createSignal<ReadonlyMap<string, CapsuleAppScreens>>(
    new Map(),
  );
  const [custom, setCustom] = createSignal<readonly CustomAppEntry[]>([]);
  const [loading, setLoading] = createSignal(false);
  /** The app list has been read once, so bar actions can find their app. */
  const [ready, setReady] = createSignal(false);
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

  // Recent apps belong to the current (instance, account).
  createEffect(
    on(
      () => props.context?.scope,
      () => {
        void props.router.recentApps().then(props.setRecent);
      },
    ),
  );

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
        setReady(true);
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
          if (generation === loadGeneration) {
            setLoading(false);
            setReady(true);
          }
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
  const recentEntries = createMemo(() => recentLauncherEntries(props.recent(), entries()));

  // An app screen's 「開き方を変える」 waits until the app list is loaded, so
  // the chooser knows the app's declared native app.
  createEffect(() => {
    const action = props.pendingAction();
    if (!action || !ready() || loading()) return;
    props.clearPendingAction();
    void props.router.handleAction(action, entries());
  });

  async function open(entry: AppLauncherEntry) {
    setStatus(undefined);
    try {
      await props.router.open(entry);
    } catch (error) {
      setStatus(mobileErrorMessage(error, `${entry.label} を開けませんでした。`));
    }
  }

  async function chooseFor(entry: AppLauncherEntry) {
    setStatus(undefined);
    await props.router.choose(entry, "change");
  }

  async function pin(entry: AppLauncherEntry) {
    setStatus(undefined);
    try {
      const context = props.context;
      const requested = await props.controller.pinEntry(entry, {
        platform: props.platform,
        resume: true,
        ...(context?.accountsOrigin ? { accountsOrigin: context.accountsOrigin } : {}),
        ...(context ? { scope: context.scope } : {}),
      });
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
        <Show when={entries().length > 0}>
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
      <Show when={recentEntries().length > 0 && !editing()}>
        <h3 class="launcher-subheading">最近使ったアプリ</h3>
        <ul class="launcher-grid launcher-recent">
          <For each={recentEntries()}>
            {(entry) => (
              <LauncherTile
                entry={entry}
                controller={props.controller}
                editing={false}
                canPin={false}
                onOpen={() => void open(entry)}
                onChoose={() => void chooseFor(entry)}
                onPin={() => undefined}
                onRemove={() => undefined}
              />
            )}
          </For>
        </ul>
        <h3 class="launcher-subheading">すべてのアプリ</h3>
      </Show>
      <ul class="launcher-grid">
        <For each={entries()}>
          {(entry) => (
            <LauncherTile
              entry={entry}
              controller={props.controller}
              editing={editing()}
              canPin={capabilities().pinShortcut}
              onOpen={() => void open(entry)}
              onChoose={() => void chooseFor(entry)}
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
      <Show when={props.chooser()} keyed>
        {(request) => (
          <OpenChooser
            request={request}
            router={props.router}
            platform={props.platform}
            openStore={props.openStore}
            {...(props.copyText ? { copyText: props.copyText } : {})}
            onClose={() => props.closeChooser()}
          />
        )}
      </Show>
    </MobilePreviewSection>
  );
}

function LauncherTile(props: {
  readonly entry: AppLauncherEntry;
  readonly controller: LauncherController;
  readonly editing: boolean;
  readonly canPin: boolean;
  readonly onOpen: () => void;
  readonly onChoose: () => void;
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
        </span>
        <span class="launcher-label">{props.entry.label}</span>
        <span class="visually-hidden">{props.entry.host}</span>
      </button>
      <Show when={props.editing}>
        <div class="launcher-tile-actions">
          <button type="button" class="text-button" onClick={() => props.onChoose()}>
            開き方
          </button>
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
