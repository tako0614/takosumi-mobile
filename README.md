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

## Development

```bash
bun install
bun run mobile:check
bun run tauri:dev
```

The host must register a public OIDC client for
`takosumi://oauth/callback` and publish its id with
`TAKOSUMI_MOBILE_OIDC_CLIENT_ID`. See the Takosumi platform deployment runbook.
