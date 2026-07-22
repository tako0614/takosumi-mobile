# AGENTS.md

`takosumi-mobile` is the independent Tauri/Solid mobile client for Takosumi.
It consumes the sibling `@takosjp/mobile-kit` package and the public Takosumi
Accounts/control API; it does not own control-plane state, runner execution, or
Cloud-only services.

- Keep the primary mobile experience native: Workspace switching, Capsule/Run
  summaries, and approval actions must use the bearer-authenticated API.
- Browser handoff is for advanced dashboard routes, not the main app shell.
- OIDC is public-client PKCE with the exact host-advertised client id. Never
  embed a client secret or a universal fallback client id.
- Keep device storage, deep links, biometrics, and native plugins product-owned
  while reusing product-neutral primitives from `../mobile-kit`.
- Run `bun run mobile:check` for ordinary changes. Native/store release gates
  are separate and require the platform SDKs and release evidence.
