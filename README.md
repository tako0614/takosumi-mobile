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

## Push relay

`relay/` holds the Web Push relay that lets apps opened inside Takosumi Mobile
receive standard Web Push through APNs and FCM (a Cloudflare Worker with D1).
See [relay/README.md](relay/README.md); `bun run deploy -- --contract` lists its
deploy surfaces.
