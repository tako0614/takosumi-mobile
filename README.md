# Takosumi Mobile

The mobile-first Takosumi client for iOS and Android. It connects to Takosumi
Cloud or an operator/self-hosted Takosumi deployment through host discovery and
public-client OIDC PKCE.

The first slice keeps the daily control loop inside the app:

- switch Workspaces;
- see installed Capsules and recent Runs;
- review approval/failed/running attention items;
- approve or cancel eligible Runs;
- hand off only advanced detail/store/settings routes to the full dashboard.

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

## Development

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
