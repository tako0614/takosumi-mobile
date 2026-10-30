import { createEffect, createMemo, createSignal, For, on, onCleanup, Show } from "solid-js";
import {
  ensureFreshMobileSession,
  formatMobilePreviewDate,
  loadMobileAccounts,
  mobileAccountIdentity,
  mobileErrorMessage,
  setActiveMobileAccount,
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
  loadRun,
  loadWorkspaceHome,
  type MobileApp,
  type MobileCapsule,
  type MobileRun,
  type TakosumiMobileHome,
} from "./api.ts";
import {
  createProductAppScreenOpener,
  createProductNativeBridge,
  createProductPushNative,
  createProductStoreOpener,
  isUnsupportedTauriDesktop,
  productPlatform,
} from "./native.ts";
import {
  appDisplayName,
  appRoleLabel,
  appStateLabel,
  isAttentionRun,
  launcherAppsFromHome,
  legacyWorkspaceOptions,
  mobileHomeTitle,
  pendingApprovalCount,
  runStatusLabel,
  runTypeLabel,
} from "./home-presentation.ts";
import { AppLauncher } from "./app-launcher.tsx";
import { fetchIconDataUrl, rasterizeIconPng } from "./app-icons.ts";
import type { AppScreenAction, AppScreenOpener } from "./app-screen-native.ts";
import { withAppScreenDeepLinks } from "./deep-links.ts";
import {
  createLauncherController,
  type LauncherController,
} from "./launcher-controller.ts";
import {
  type ChooserRequest,
  createOpenRouter,
  type OpenContext,
  openContextFromSession,
  type OpenRouter,
  type RecentApp,
} from "./open-routing.ts";
import { productAdapter } from "./product.ts";
import { withAccountRemovalHook } from "./push-account-hook.ts";
import { deviceLanguage } from "./push-instance.ts";
import { createPushManager, pushOrigins, type PushManager } from "./push-manager.ts";
import type { NotificationReceived, NotificationTap, PushNative } from "./push-native.ts";
import { normalizeRelayOrigin } from "./push-relay.ts";
import {
  accountForTap,
  createPendingNotification,
  routeNotification,
  tapIsForSession,
  type PendingNotifications,
} from "./push-routing.ts";
import { PushSettings } from "./push-settings.tsx";
import { RunScreen } from "./run-screen.tsx";
import { authorizeRunMutation } from "./security.ts";
import {
  createDeepLinkOpener,
  readKnownTakosumiOrigins,
  rememberTakosumiOrigins,
  savedAccountOrigins,
} from "./takosumi-origins.ts";
import { takosumiOrigins } from "./url-policy.ts";
import "./styles.css";

/** Takosumi's own notifications, as the home needs them. */
interface NotificationWiring {
  readonly manager: PushManager;
  readonly pending: PendingNotifications;
  /** Subscribe to notifications that arrived while the app is open. */
  readonly onReceived: (handler: (event: NotificationReceived) => void) => () => void;
}

/** How apps open: the router, its chooser, recent apps and bar actions. */
interface OpenRouting {
  readonly router: OpenRouter;
  readonly platform: string;
  readonly openWeb: (url: string) => Promise<void>;
  readonly openStore: (url: string) => Promise<void>;
  readonly setContext: (context: OpenContext | undefined) => void;
  readonly context: () => OpenContext | undefined;
  readonly chooser: () => ChooserRequest | undefined;
  readonly closeChooser: () => void;
  readonly recent: () => readonly RecentApp[];
  readonly setRecent: (recent: readonly RecentApp[]) => void;
  readonly pendingAction: () => AppScreenAction | undefined;
  readonly clearPendingAction: () => void;
}

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
  {
    // The unread count is Takosumi's own data (Runs waiting for this
    // account's approval), never a count of notifications.
    label: "承認待ち",
    value: (home) => pendingApprovalCount(home),
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
  const baseBridge = createProductNativeBridge();
  const opener = createProductAppScreenOpener(baseBridge);
  const platform = productPlatform();
  const openWeb = (url: string) => opener.openWeb(url);
  const openStore = createProductStoreOpener(baseBridge);
  const launcher = createLauncherController({
    opener,
    fetchIcon: (url) => fetchIconDataUrl(url),
    rasterize: (source) => rasterizeIconPng(source),
  });
  const [context, setContext] = createSignal<OpenContext>();
  const [chooser, setChooser] = createSignal<ChooserRequest>();
  const [recent, setRecent] = createSignal<readonly RecentApp[]>([]);
  const [pendingAction, setPendingAction] = createSignal<AppScreenAction>();
  const router = createOpenRouter({
    opener,
    openInApp: (target, options) => launcher.openTarget(target, options),
    openWeb,
    ...(baseBridge.storage ? { store: baseBridge.storage } : {}),
    platform,
    context,
    showChooser: setChooser,
    onRecentChanged: setRecent,
  });
  const deepLinks = createDeepLinkOpener({
    openUrl: (url, options) => launcher.openUrl(url, options),
    platform,
    currentOrigins: () => context()?.accountsOrigins,
    knownOrigins: () =>
      readKnownTakosumiOrigins({ store: baseBridge.storage, adapter: productAdapter }),
    hasSavedAccounts: async () =>
      (await savedAccountOrigins(baseBridge.storage, productAdapter)).length > 0,
    onError: (error) =>
      console.warn(mobileErrorMessage(error, "アプリの画面を開けませんでした。")),
  });
  const routing: OpenRouting = {
    router,
    platform,
    openWeb,
    openStore,
    setContext: (next) => {
      setContext(next);
      // A link that arrived before the session was restored opens now.
      if (next) void deepLinks.sessionReady();
    },
    context,
    chooser,
    closeChooser: () => setChooser(undefined),
    recent,
    setRecent,
    pendingAction,
    clearPendingAction: () => setPendingAction(undefined),
  };
  // An app screen's bar (開き方を変える, a missing or unverified native app)
  // leaves an action for the shell; take it now and whenever one arrives.
  const takeAction = () => {
    void opener
      .takePendingAction()
      .then((action) => {
        if (action) setPendingAction(action);
      })
      .catch(() => undefined);
  };
  void opener.onActionPending(takeAction).catch(() => undefined);
  takeAction();
  const pushNative = createProductPushNative();
  // Signing out of an instance removes its pusher and relay subscription
  // while the session it had can still authenticate.
  const nativeBridge = withAccountRemovalHook(
    withAppScreenDeepLinks(baseBridge, (link) => {
      if (!link.ok) {
        console.warn(`Ignored app screen link: ${link.error}`);
        return;
      }
      // A link names a page to show inside Takosumi, judged with every known
      // Takosumi origin. It neither resumes nor becomes the app's remembered
      // last page: only pages the person reaches from the app itself are.
      void deepLinks.open(link.url);
    }),
    productAdapter,
    (session) => push.manager.accountRemoving(session),
  );
  const push = createNotificationWiring(pushNative, nativeBridge);
  const render = () => renderShell({ nativeBridge, launcher, opener, routing, push });

  // A tapped notification names its instance and account: make that account
  // the active one, then (re)start the shell, which restores its session and
  // opens the notification's page. Taps that arrive while the shell runs for
  // the same account open at once.
  const takeTap = async (tap: NotificationTap | undefined): Promise<boolean> => {
    if (!tap) return false;
    const list = await loadMobileAccounts({ adapter: productAdapter, nativeBridge });
    const account = accountForTap(tap, list.accounts);
    if (!account) return false;
    const switching = list.activeId !== account.id;
    if (switching) {
      await setActiveMobileAccount({ adapter: productAdapter, nativeBridge, accountId: account.id });
    }
    push.pending.set({
      identity: mobileAccountIdentity(account),
      ...(tap.navigate ? { navigate: tap.navigate } : {}),
    });
    return switching;
  };
  void (async () => {
    // Cold start: the tap that launched the app is waiting natively.
    await takeTap(await pushNative?.takePendingTap().catch(() => undefined)).catch(() => false);
    render();
    if (!pushNative) return;
    await pushNative
      .onTap(() => {
        void pushNative
          .takePendingTap()
          .then(takeTap)
          .then((switched) => {
            if (switched) render();
          })
          .catch((error) => console.warn(mobileErrorMessage(error, "通知を開けませんでした。")));
      })
      .catch(() => undefined);
    await pushNative.onTokenRefresh((token) => void push.manager.tokenRefreshed(token)).catch(() => undefined);
    // Subscriptions of accounts that are gone, orphaned keys, failed deletions.
    const accounts = await loadMobileAccounts({ adapter: productAdapter, nativeBridge }).catch(() => undefined);
    if (accounts) await push.manager.reconcile(accounts.accounts).catch(() => undefined);
  })();
}

function createNotificationWiring(native: PushNative | undefined, nativeBridge: NativeBridge): NotificationWiring {
  const env = import.meta.env as Record<string, string | undefined>;
  const manager = createPushManager({
    native,
    relayOrigin: normalizeRelayOrigin(env.VITE_TAKOSUMI_PUSH_RELAY_ORIGIN),
    fetch: (input, init) => globalThis.fetch(input, init),
    store: nativeBridge.storage,
    secureStore: nativeBridge.secureStore,
    freshSession: (session) =>
      ensureFreshMobileSession({ adapter: productAdapter, nativeBridge, session }),
    lang: () => deviceLanguage(typeof navigator === "undefined" ? undefined : navigator.language),
  });
  const receivedHandlers = new Set<(event: NotificationReceived) => void>();
  void native
    ?.onReceived((event) => {
      for (const handler of receivedHandlers) handler(event);
    })
    .catch(() => undefined);
  return {
    manager,
    pending: createPendingNotification(),
    onReceived(handler) {
      receivedHandlers.add(handler);
      return () => receivedHandlers.delete(handler);
    },
  };
}

function renderShell(input: {
  readonly nativeBridge: NativeBridge;
  readonly launcher: LauncherController;
  readonly opener: AppScreenOpener;
  readonly routing: OpenRouting;
  readonly push: NotificationWiring;
}) {
  const { nativeBridge, launcher, opener, routing, push } = input;
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
      accountsLabel: "接続しているTakosumi",
      savedAccountsLabel: "保存したTakosumi",
      addAccountLabel: "別のTakosumiを追加",
      discoveredWhileSignedInHeading: "別のTakosumiが見つかりました",
      signInToAddLabel: "サインインして追加",
      // Every Takosumi accepts the built-in `takosumi-mobile` client unless its
      // operator turned it off (TAKOSUMI_MOBILE_OIDC_BUILTIN_CLIENT=false).
      signInUnavailableMessage:
        "このTakosumiはTakosumi Mobileのサインインを有効にしていません。運用者に確認してください。",
      signInUnavailableHelpUrl: "https://takosumi.com/docs/reference/api",
      signInUnavailableHelpLabel: "運用者向けの手順",
    },
    metrics,
    hostActions: actions,
    renderHomeExtra: ({ home, session, openHostRoute }) => (
      <TakosumiControlHome
        home={home}
        session={session}
        nativeBridge={nativeBridge}
        openHostRoute={openHostRoute}
        launcher={launcher}
        opener={opener}
        routing={routing}
        push={push}
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
  launcher: LauncherController;
  opener: AppScreenOpener;
  routing: OpenRouting;
  push: NotificationWiring;
}) {
  const [current, setCurrent] = createSignal(props.home);
  const launcherApps = createMemo(() => launcherAppsFromHome(current()));
  const [activeSession, setActiveSession] = createSignal(props.session);
  const [loading, setLoading] = createSignal(false);
  const [status, setStatus] = createSignal<string>();
  /** The Run screen: a Run id and the dashboard page it mirrors. */
  const [runScreen, setRunScreen] = createSignal<{ readonly runId: string; readonly url: string }>();

  createEffect(() => setCurrent(props.home));
  createEffect(() => setActiveSession(props.session));
  // Choices, recent apps and last pages follow the current instance/account.
  createEffect(() => {
    const session = activeSession();
    void (async () => {
      const store = props.nativeBridge.storage;
      // This instance's Accounts and instance origins join the known list.
      await rememberTakosumiOrigins(store, takosumiOrigins(session));
      const known = await readKnownTakosumiOrigins({ store, adapter: productAdapter });
      const context = await openContextFromSession(session, known);
      if (activeSession() === session) props.routing.setContext(context);
    })().catch(() => props.routing.setContext(undefined));
  });

  // A tapped notification for this instance and account opens here: a Run in
  // the Run screen, any other page of the instance in the browser view.
  const openPendingNotification = () => {
    const session = activeSession();
    const pending = props.push.pending.takeFor(session);
    if (!pending) return;
    const route = routeNotification(pending.navigate, pushOrigins(session));
    if (route.kind === "run") {
      setRunScreen({ runId: route.runId, url: route.url });
    } else if (route.kind === "page") {
      void openTakosumiPage(route.url);
    }
    void reload();
  };
  createEffect(on(activeSession, () => openPendingNotification()));
  onCleanup(props.push.pending.subscribe(openPendingNotification));
  // A notification for this account while the app is open: the home's counts
  // come from Takosumi, so read them again.
  onCleanup(
    props.push.onReceived((event) => {
      if (tapIsForSession(event, activeSession())) void reload();
    }),
  );

  /** A Takosumi page outside sign-in: the browser view (#7). */
  async function openTakosumiPage(url: string) {
    const accountsOrigins = props.routing.context()?.accountsOrigins ?? pushOrigins(activeSession());
    try {
      await props.launcher.openUrl(url, { platform: props.routing.platform, accountsOrigins: [...accountsOrigins] });
    } catch (error) {
      setStatus(mobileErrorMessage(error, "ページを開けませんでした。"));
    }
  }

  function openRun(run: MobileRun) {
    // The dashboard is served where the instance's notifications point: its
    // Accounts (issuer) origin, which is usually the instance origin too.
    const origins = pushOrigins(activeSession());
    const origin = origins[1] ?? origins[0]!;
    setRunScreen({ runId: run.id, url: `${origin}/runs/${encodeURIComponent(run.id)}` });
  }

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

  async function approve(run: MobileRun): Promise<boolean> {
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
      return true;
    } catch (error) {
      setStatus(mobileErrorMessage(error, "承認できませんでした。"));
      setLoading(false);
      return false;
    }
  }

  async function cancel(run: MobileRun): Promise<boolean> {
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
      return true;
    } catch (error) {
      setStatus(mobileErrorMessage(error, "キャンセルできませんでした。"));
      setLoading(false);
      return false;
    }
  }

  return (
    <div class="takosumi-control-home">
      <Show when={legacyWorkspaceOptions(current()).length > 1}>
        <label class="workspace-picker">
          <span>スペース</span>
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

      <Show when={runScreen()}>
        {(screen) => (
          <RunScreen
            runId={screen().runId}
            host={new URL(activeSession().hostUrl).host}
            loadRun={(runId) => loadRun(activeSession(), runId)}
            appName={capsuleName}
            approve={approve}
            cancel={cancel}
            openDashboard={() => void openTakosumiPage(screen().url)}
            onClose={() => setRunScreen(undefined)}
          />
        )}
      </Show>

      <AppLauncher
        session={activeSession()}
        apps={launcherApps()}
        store={props.nativeBridge.storage}
        controller={props.launcher}
        opener={props.opener}
        router={props.routing.router}
        platform={props.routing.platform}
        context={props.routing.context()}
        chooser={props.routing.chooser}
        closeChooser={props.routing.closeChooser}
        recent={props.routing.recent}
        setRecent={props.routing.setRecent}
        pendingAction={props.routing.pendingAction}
        clearPendingAction={props.routing.clearPendingAction}
        openStore={props.routing.openStore}
        {...(props.nativeBridge.writeClipboardText
          ? {
              copyText: (text: string) =>
                props.nativeBridge.writeClipboardText!({ text, label: "URL" }),
            }
          : {})}
      />

      <MobilePreviewSection
        title="要対応"
        detail={attentionDetail(attentionRuns(), pendingApprovalCount(current()))}
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
                        onClick={() => openRun(run)}
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

      <PushSettings session={activeSession()} manager={props.push.manager} />

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
                        onClick={() => openRun(run)}
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

function attentionDetail(runs: readonly MobileRun[], approvals: number): string {
  if (!runs.length) return "なし";
  const failed = runs.filter((run) => run.status === "failed").length;
  const parts = [
    ...(approvals ? [`承認待ち ${approvals}件`] : []),
    ...(failed ? [`失敗 ${failed}件`] : []),
  ];
  return parts.length ? parts.join("・") : `${runs.length}件`;
}
