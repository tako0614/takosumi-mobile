# Takosumi Mobile

The mobile-first Takosumi client for iOS and Android. It connects to Takosumi
Hosted or an operator/self-hosted Takosumi deployment through host discovery and
public-client OIDC PKCE.

## What you can do

The home lists your installed apps as icons, and each opens like an app:

- launchable screens come from each app's view
  (`GET /api/v1/capsules/{capsuleId}/app`, the Interface `display.icon` and URL);
  you can also add any https URL, kept only on the device;
- an app opens in its own native webview (the `app-screen` plugin) under a thin
  bar that always shows the current host, with back, reload, share, open in the
  browser and close; all app screens share one sign-in profile;
- on Android, each app gets its own entry in Recents and can be added to the
  home screen; `takosumi://app?url=<https URL>` opens an app screen directly.

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
imports the public session contract directly from that checkout.

```bash
bun install
bun run mobile:check
bun run tauri:dev
```

The host must register a public OIDC client for
`takosumi://oauth/callback` and publish its id with
`/.well-known/takosumi`. Its exact allowed scope set is:

```text
openid offline_access workspaces:read capsules:read runs:read runs:approve runs:cancel
```

The host derives the published client id from the one registered public client
whose redirect URI is exactly `takosumi://oauth/callback`; the app has no
fallback client id. See the Takosumi platform deployment runbook.

## Push relay

`relay/` holds the Web Push relay that lets apps opened inside Takosumi Mobile
receive standard Web Push through APNs and FCM (a Cloudflare Worker with D1).
See [relay/README.md](relay/README.md); `bun run deploy -- --contract` lists its
deploy surfaces.
