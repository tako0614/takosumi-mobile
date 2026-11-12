# Takosumi Mobile

The Tauri Takosumi client for Android, iOS, Windows, macOS and Linux. A fresh
launch opens the shared connection picker. Choose a Takosumi instance — Hosted,
operator-managed or self-hosted — and use its Web sign-in, home and apps.
The previous iOS/Android client and its saved credentials remain available
through an explicit choice.

## Shared Web home

The Web client retains connection selection while opening Takosumi's own Web
sign-in, app home and management screens. Apps linked by the Web home open in
plain native WebViews within the same browsing profile. The main screen's menu
offers **アプリホームへ** and **接続先を切り替える**; a popup closes back to its
opener, and the current site's host stays visible. Pages receive no
Tauri IPC, native credential or management-API adapter.

Browser preview: `bun run dev`, then open `http://localhost:1450/`. The preview
uses the ordinary browser; it does not prove native engine behavior.

Native development and debug builds open the same Web home by default:

```bash
bunx tauri android dev
bunx tauri android build --debug
# On a macOS host with the iOS SDK:
bunx tauri ios dev
# Desktop (the same UI, connection records and native request):
bunx tauri dev
bunx tauri build --debug --no-bundle
```

On Android/iOS, **以前のモバイル版を使っていた方 → 保存済みログインを開く**
opens the preserved mobile client. Its **新しいTakosumiホームへ** link returns
to the shared Web home, including before signing in. This explicit choice is
remembered separately when preferences are available. Switching reloads the
local document and replaces its history entry, so Back does not undo the choice.
Existing native sessions
and credentials are neither imported nor deleted; they retain their previous
unlock and authorization guards. The Web path never initializes that client.
For diagnostics, `?web-client=0` / `?web-client=1` still select either entry.
The existing `tauri.web-client.conf.json` overlay can force Web mode for tests.
Only connection names, https site roots and opaque profile IDs are stored by the Web shell;
callback URLs, credentials, paths, queries and fragments are refused as
connection inputs. Removing a connection removes that preference, not site data
or the site's session. Sign-out remains a Web operation.

New Web connections each use an isolated persistent profile; adding the same
site again creates a separate login, with a distinct label even if no name was
entered. Android requires a WebView engine supporting AndroidX `MULTI_PROFILE`;
iOS requires 17 or later. Unsupported isolation reports an error without opening
the shared jar. Popups use the opener's profile. Within a profile, normal Web
origin/domain cookie and storage rules apply. The old v1 connection preference
is retained, and its entries continue using the existing shared profile; no
cookie or native credential is copied. Subsequent preference edits use a v2 key.

The ordinary browser preview cannot allocate these native profiles, so opening
a newly isolated connection there reports that limitation. Android actual-WebView profile tests are described
in [native-tests/README.md](native-tests/README.md); Swift policy tests compile
in CI and are distinct from iOS WebView qualification.

Desktop uses Tauri 2 with a raw Wry content adapter: Chromium-based WebView2 on
Windows, WebKit on macOS and Linux. macOS requires 14 or later for persistent
profile stores. Linux uses GTK containers for both X11 and Wayland. Remote views
are never registered as Tauri webviews. Their window menu provides Back, Reload,
App home, Connection picker and Close; the current https origin is visible in
the native title bar. Windows/Linux keep profile directories separate from the
trusted shell; macOS uses dedicated UUID data stores. Popups inherit their
opener's native environment/store. The shell and mobile/desktop adapters share
`WebHome`, connection persistence, profile identity and validated `OpenRequest`.
The explicit `desktop-cef` feature keeps the Tauri 2 shell and mobile plugins
while opening each connection in its own guarded CEF browser process on Linux,
Windows and macOS. On Linux, the browser is packaged under the bundle's `cef/`
resources; see
[Linux packaging](native-tests/cef-candidate/README.md#linux-packaging-desktop-cef).
On Windows the same adapter and a Job Object guard compile and are tested in
CI, with the browser in Chromium's sandbox (CEF's bootstrap with the browser
as its DLL), and CI jobs run the actual App (its home in WebView2) through
them end to end against a local fixture site, also from unsigned per-user
NSIS installers that bundle the browser, the guard and the CEF runtime under
`cef/`; see
[Windows guard and adapter](native-tests/cef-candidate/README.md#windows-guard-and-adapter),
[Windows actual App (W3)](native-tests/cef-candidate/README.md#windows-actual-app-w3),
[Windows packaging (W6)](native-tests/cef-candidate/README.md#windows-packaging-w6)
and [Windows sandbox (W7)](native-tests/cef-candidate/README.md#windows-sandbox-w7).
On macOS the per-profile browser builds as an app bundle with its helper apps
and runs on its own in CI; see
[macOS browser process (M1)](native-tests/cef-candidate/README.md#macos-browser-process-m1).
Its guard/adapter are covered by M2 component tests. The explicit
[macOS actual App fixture (M3)](native-tests/cef-candidate/README.md#macos-actual-app-fixture-m3)
adds the shared home/profile flow on macOS 14+; its native qualification is
limited to anonymous Excalidraw/Photopea fixtures. Both passed all three
launches in [CI43195505965](https://github.com/tako0614/takosumi-mobile/actions/runs/43195505965).
The intermittent ordinary-shutdown failure remains unresolved: a later
unbundled Photopea run returned normally from the browser root but left
helpers requiring forced cleanup.
The [macOS packaging candidate (M4)](native-tests/cef-candidate/README.md#macos-packaging-m4)
places that browser and guard in the Tauri App's own resources. The local
debug `.app` builds and embedded product-browser sandbox smoke passed.
Packaged Excalidraw and Photopea each passed the three-launch home/profile
flow with normal inner/outer receipts in
[CI43202312305](https://github.com/tako0614/takosumi-mobile/actions/runs/43202312305).
Signing and distribution remain separate release work.

On Linux/Windows with `desktop-cef`, opening a saved connection after its browser
crashes can offer **保存データを保持して開き直す** on the home. This explicit action
keeps that connection's UUID and site-data directory and starts a new browser;
it does not clear the profile. It is available only after the original guard
proves that all its processes ended, with no unresolved ownership errors or
live Chromium lock. The original abnormal receipt is retained privately under
the profile's `recoveries/` directory. Unsaved changes and writes interrupted by
the crash may be lost. Ordinary open, clear and quit keep their stricter normal
exit checks. This recovery applies to a browser tracked by the still-running
App; it does not qualify recovery after an App/OS crash. macOS and mobile do not
advertise this command. The synthetic-site Windows same-App browser-fault
case passed in [CI43218039839](https://github.com/tako0614/takosumi-mobile/actions/runs/43218039839/job/131182189815).
Installed-package recovery and interrupted-write integrity remain unqualified.

The ordinary Windows product entry now returns through the event loop before
exiting, as the actual-App fixture already does, allowing remaining shell
WebView2 controllers to drop. The unsigned installed-product smoke passed in
[CI43272033643](https://github.com/tako0614/takosumi-mobile/actions/runs/43272033643/job/131370792374):
eight seconds up, normal App exit and an empty job within the original ten-second
bound. The prior four-process remainder is absent in this run. This is a bounded
first-launch/close check; distribution and updates remain release work.

The GTK content adapter routes HTTPS-origin Blob exports through WebKit's
native download service while keeping them out of page navigation. Its
Excalidraw/Photopea file checks and remaining platform limits are described in
[native tests](native-tests/README.md).

The iOS Web-home adapter now hands download actions and attachment responses
to the original WKDownload, with native confirmation, transfer cancellation
and OS export. It keeps the originating profile and does not reopen the URL
in Safari or copy cookies. The standalone native host compiling these product
sources passed on an iOS 26.5 / iPhone 11 Simulator: original Cookie/Blob
exports, profile isolation, close during transfer/export, and owned temporary
file cleanup. A later run failed after Save to Files while the real Files
picker remained visible; export reliability is still open. Whole-App UI,
real IdP, physical-device and background/crash behavior remain unqualified.
See the
[download contract](src-tauri/plugins/app-screen/README.md#ios-web-home-downloads)
and [runtime fixture](native-tests/README.md#ios-download-runtime-qualification).

Web homes remember an eligible last page separately for each connection and
profile, including another app's https origin. Native document outcomes gate
this metadata: provisional loads and transport failures do not replace a working
page, while successful same-document routes can be resumed. Credential callback
queries, fragments and transient sign-in paths are excluded. The shared URL and
navigation-transition fixtures keep Rust, Kotlin and Swift rules aligned.
Unknown failure identities keep the saved metadata rather than qualify a new
page. OS runtime qualification is still required for these native callbacks.

Wry's default content messaging is removed on GTK/WK before navigation.
GTK also disconnects the raw manager's native message signal: unregistering the
JS channel alone leaves a Wry callback that retains the view and its context.
The trusted shell's IPC stays active until actual GTK widget destruction;
document reloads and cancelled close requests keep their normal connection.
On Windows the WebMessage transport is disabled before navigation; Wry's inert
`window.ipc` JS stub may remain, with no application/Tauri handler behind it.
No remote page is granted a capability, custom protocol, OS opener or host object.

Current limits: site-data deletion, OS-by-OS cold-restoration qualification,
notification/deep-link routing and distribution/update are unfinished. Web
authentication (including third-party IdPs), popups and file operations still
need Android/iOS device qualification; some IdPs refuse embedded browsers. No
external browser cookies or tokens are copied to bypass that restriction. A
desktop native runtime still needs OS-by-OS qualification. The preserved mobile
entry refuses desktop authentication and offers a return link to Web home.
This source does not claim the full App goal complete.

## Preserved mobile client

### What you can do

The home lists your installed apps as icons, and each opens like an app:

- launchable screens come from each app's view
  (`GET /api/v1/capsules/{capsuleId}/app`, the Interface `display.icon` and URL);
  you can also add any https URL, kept only on the device;
- tapping an app opens it the way you chose on this device, else in the app's
  own native app when the app declares one and the instance verified it, else
  inside Takosumi Mobile; the chooser (「開き方」) offers the native app (and
  its store page), the browser, the browser view (Custom Tabs /
  `SFSafariViewController`) and the screen inside Takosumi, and can remember
  the choice. A native app the instance did not verify is never opened without
  asking: the chooser shows it as 「未確認のアプリ」 (or its checked name) with
  the app, host and store it really goes to;
- inside Takosumi an app opens in its own native webview (the `app-screen`
  plugin) under a thin bar that always shows the current host, with back,
  reload and a menu (share, the browser view, the browser, the declared native
  app, 開き方を変える, switch apps, close); all app screens share one sign-in
  profile. Pages of other sites, such as a Google sign-in, open in the browser
  view instead, and closing it returns to the app;
- the home shows your recently used apps, and an app reopens at the page you
  left (remembered per instance and account, never with credentials in it);
- on Android, each app gets its own entry in Recents and can be added to the
  home screen; `takosumi://app?url=<https URL>` opens an app screen directly.
- app screens have no access to Takosumi Mobile itself: no Tauri capability,
  no IPC and no JavaScript bridge.

Instances and accounts:

- add an instance by URL, QR, a recent host, or a
  `takosumi://connect?host_url=https://X` link; a connect link that arrives
  while you are signed in elsewhere is shown with "サインインして追加", not hidden;
- each (instance, account) keeps its own session in the device's secure store;
  the top of the home shows the current instance and account, and tapping it
  switches, adds another instance, or signs one out (its tokens are revoked at
  that instance);
- the home, approvals and app list always come from the current instance, and
  a token for one instance is never sent to another.

Notifications from Takosumi itself (TASK-0119 step 7):

- each (instance, account) has its own switch on the home (「通知」), and the
  app asks once after signing in to a new one; turning it on asks for the OS
  notification permission;
- the instance tells this device about Runs waiting for approval, failed Runs,
  finished installs and other events of its own (takosumi
  `docs/reference/api.md`「Takosumi Mobile の通知の登録」). It sends standard
  Web Push (RFC 8291 `aes128gcm`, VAPID) to the push relay below, which
  forwards the still-encrypted message through FCM / APNs. The device decrypts
  it with a key that never leaves it (Android: wrapped by the Android Keystore;
  iOS: the Keychain, in an access group a Notification Service Extension can
  share) and shows the Declarative Web Push message on that instance's own
  notification channel. Only a message that decrypts with a subscription's
  key may switch accounts or open a page (the relay's envelope is not
  authenticated): one this device cannot read shows generic text and opens
  just the app, and one for a subscription it does not have is dropped;
- tapping a notification switches to its instance and account, also when the
  app was not running, and opens the page: a Run opens in the app's Run
  screen (approve / cancel there), any other page of the instance in the
  browser view;
- signing out of an instance removes its pusher and relay subscription; when
  the instance's key changes (its operator rotated the secret), the app
  subscribes again on the next launch; removing this device in the instance's
  account settings turns the switch off;
- the home's 「承認待ち」 count comes from Takosumi's own Run list, not from
  notifications.

Apps opened inside Takosumi Mobile do not get Web Push (the webviews have no
Push API); the chooser shows how to get an app's own notifications.

It also keeps the daily control loop inside the app:

- switch Workspaces;
- see installed Capsules and recent Runs;
- review approval/failed/running attention items;
- approve or cancel eligible Runs;
- hand off only advanced detail/store/settings routes to the full dashboard.

## How it behaves

The shell follows the control API's opaque pagination cursors, remembers the
selected Workspace per operator host, and uses the canonical
`personal | organization` Workspace owner kind. Restored sessions remain locked
until native biometric/device authentication succeeds.

Run approval and cancellation are least-privilege mutations. The app performs
an intent-specific confirmation, native biometric authentication, and token
refresh before sending a bearer request. Backup restore uses a dedicated
destructive confirmation and cannot pass through the ordinary approval helper.
Desktop Tauri builds fail closed and never load stored credentials; the product
is distributed for iOS and Android.

## Getting started

A Takosumi source checkout is also required at `../takosumi`; `src/api.ts`
and `src/app-clients.ts` import the public session contract and the native
app declaration contract (`contract/interface-clients.ts`) directly from that
checkout.

```bash
bun install
bun run mobile:check
bun run tauri:dev
```

For Android, initialize the native project with `bun run tauri:android:init`,
then use `bun run tauri:android:dev` to select and run on a device or emulator.
The default command does not open Android Studio or force a network host.
Arguments are forwarded to the installed Tauri CLI:

```bash
bun run tauri:android:dev --help
bun run tauri:android:dev "device name" --host 192.0.2.10
bun run tauri:android:dev --open --host 192.0.2.10
```

Replace the example device name and address with your actual device and a
reachable development-machine address. `--open` selects Android Studio instead
of directly running on the device. `--host` is optional; when supplied, the
shared Vite configuration uses Tauri's `TAURI_DEV_HOST` for the server and HMR.
Do not use `0.0.0.0` as the address a device connects to. CLI argument checks and
`bun run doctor` do not establish device launch or LAN connectivity.

Every Takosumi instance includes the built-in public OIDC client
`takosumi-mobile` (redirect exactly `takosumi://oauth/callback`, PKCE S256) and
publishes it as `oidcClientId` in `/.well-known/takosumi`; no per-instance
registration is needed. The app requests exactly the scopes that client is
registered for:

```text
openid profile offline_access capsules:read capsules:write
```

`capsules:write` covers Run approval and cancellation. Each authorization shows
an instance consent screen, because another app could claim the same custom
scheme. API calls use the instance's advertised `apiBaseUrl` (accepted only on
the instance's own origin). An instance whose operator turned the built-in
client off (and selected no other) advertises no `oidcClientId`; the app then
says that the instance has not enabled Takosumi Mobile sign-in and links to the
operator instructions. The app has no fallback client id.

## Push relay

`relay/` holds the Web Push relay that carries Takosumi's own notifications to
Takosumi Mobile through APNs and FCM (a Cloudflare Worker with D1).
See [relay/README.md](relay/README.md); `bun run deploy -- --contract` lists its
deploy surfaces.

The app talks to one relay, fixed at build time:

```bash
VITE_TAKOSUMI_PUSH_RELAY_ORIGIN=https://<relay origin> bun run tauri:android:build
```

Without it the 「通知」 section says that this build has no relay. Each instance
accepts devices only for relay origins its operator listed in
`TAKOSUMI_NOTIFICATION_PUSH_RELAY_ORIGINS`, so the operator lists the same
origin. The production relay domain is not decided yet; the staging relay
(`takosumi-push-relay-staging`, sink delivery) works for development.

Native setup the build does not provide:

- Android: a Firebase project for `com.takosumi` and its
  `google-services.json` in `src-tauri/gen/android/app/` (with the Google
  Services Gradle plugin, as `bun run tauri:android:init` wires it). Without
  it the app runs and reports that this build cannot receive notifications.
  The relay sends FCM registration tokens (`message.token`), which the app
  takes from `FirebaseMessaging.getToken()`.
- iOS: the Push Notifications capability (`aps-environment`). The APNs
  environment the relay is told comes from Info.plist
  `TakosumiApnsEnvironment` (`development` / `production`, per build
  configuration), else the embedded provisioning profile's `aps-environment`,
  else production (App Store / TestFlight). For the Notification Service
  Extension (not built yet): a shared Keychain access group listed in
  `keychain-access-groups` and in Info.plist as
  `TakosumiPushKeychainAccessGroup` (for example
  `$(AppIdentifierPrefix)com.takosumi.push`); the extension links the
  plugin's `TakosumiPushKeychain` target, which holds the keys and the same
  decryption.

The native side is `src-tauri/plugins/push` (`takosumi-push`): the token, one
P-256 key pair and `auth` per subscription, decryption
(`WebPushCrypto.kt`, checked against the RFC 8291 Appendix A vector and the
shared `web-push-cases.json`), the notification channels and taps. A tap
intent carries only an opaque token the plugin issued for that notification,
and a tap the shell has not taken within five minutes is not used. CI runs the
decryption tests on the plain JVM (`src-tauri/plugins/push/jvm-tests`), the
iOS decryption (`TakosumiWebPush.swift`) against the same table with
swift-crypto on Linux (`src-tauri/plugins/push/ios/WebPushTests`), and the
plugins' `cargo test`, none of which `bun run check` covers.
