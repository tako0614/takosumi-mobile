# Takosumi Mobile

The mobile-first Takosumi client for iOS and Android. Like a Mastodon client,
it connects to any Takosumi instance — Takosumi Hosted or an operator or
self-hosted deployment — through `/.well-known/takosumi` discovery and
public-client OIDC PKCE, and keeps several instances and accounts signed in at
once.

## What you can do

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

A shared-account host (Takosumi Hosted, app.takosumi.com) works differently:
people sign in there only through the host's private relying party, and the
host accepts no bearer token. It advertises no `oidcClientId` but
`shared_account_sign_in` instead. The app then signs in at that issuer (the one
the browser uses) as the Takosumi Mobile client registered there, with PKCE,
a nonce and a consent screen, and the host's HTTPS redirect page hands the
response back to `takosumi://oauth/callback`. The app does not redeem the code:
it sends `{code, code_verifier, nonce}` to the host's `session_exchange`, whose
relying party redeems it and returns a session of the browser's kind. The app
keeps that session in Stronghold like any other and sends it only to that host,
in `x-takosumi-native-session` (never as `Authorization`). Signing out revokes
it at the host; when it expires the app asks for a new sign-in
(`src/shared-account-sign-in.ts`, takosumi `docs/reference/api.md`).

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
