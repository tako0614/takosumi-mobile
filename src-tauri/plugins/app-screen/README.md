# Takosumi mobile app screen plugin

This product-owned Tauri plugin opens an installed app (or a URL the person
added on the device) in its own native webview: `WKWebView` on iOS and
`android.webkit.WebView` on Android. The webview is separate from the Tauri
shell's own webview and sits under a thin native bar:

- the current host, always visible (there is no address field, so a page
  cannot pretend to be another site);
- back, reload and close;
- a menu with share, ブラウザの画面で開く (the browser view), open in the
  browser, 「{name}で開く」 when the app declares a native app, 開き方を変える,
  switch apps and, on Android, add to the home screen.

It also hands a screen to its declared native app (`open_client`) and opens
pages in the browser view (`open_browser_view`): Custom Tabs on Android,
`SFSafariViewController` on iOS.

Design: `takos-control/docs/platform/takosumi-app-clients.md` §3 and §5
(TASK-0119 step 3). App screens receive no Web Push; the iOS widget and App
Intents come later.

## Commands

| Command | Payload | Result |
| --- | --- | --- |
| `capabilities` | none | `{ pinShortcut }` (Android launchers that can pin) |
| `open` | `{ key, url, title, iconPng?, profile: "shared", accountsOrigin?, scope?, resume?, client? }` | opens or returns to the screen |
| `pin_shortcut` | same as `open` | `{ requested }`; always `false` on iOS |
| `open_client` | `{ platform, package?, appLink?, universalLink?, open? }` | `{ result: "opened" \| "not_installed" }` |
| `open_browser_view` | `{ url, toolbarColor? }` | `{ result: "browser-view" \| "browser" }` |
| `take_pending_action` | none | `{ action? }`: `{ kind: "choose", key, url, reason }` left by a bar |

`key` is the screen's stable identity (`app:<capsuleId>:<interfaceId>`,
`custom:<id>` or `origin:<origin>`). `iconPng` is a base64 PNG the shell drew
from the Interface `display.icon` (SVG and glyphs are rasterized in the shell;
the native side only decodes PNG). `accountsOrigin` is the instance's Accounts
(OIDC issuer) origin, `scope` the (instance, account) the last page is kept
for, `resume` reopens at that page, and `client` the declared native app the
bar offers (a verified one with its links, an unverified one with its label
only). Rust, Kotlin and Swift each re-check that the URL is https without user
info and not one of the shell's own hosts, that `profile` is `shared`, and the
origin, scope, package, links and schemes (the schemes Takosumi refuses for
native apps, `contract/interface-clients.ts`, are refused here too).

`open_client` pins the target: on Android `ACTION_VIEW` with `setPackage`,
the App Link first with `FLAG_ACTIVITY_REQUIRE_NON_BROWSER`, then the custom
scheme, and `ActivityNotFoundException` is `not_installed` (no `<queries>`,
no `QUERY_ALL_PACKAGES`); on iOS `UIApplication.open` with
`universalLinksOnly` first, then the custom scheme, and `false` from both is
`not_installed` (no `canOpenURL`). The shell never opens an unverified client
without the person's confirmation.

An app screen's bar reaches the shell only through `take_pending_action`: the
bar stores the action (開き方を変える, a native app that is not installed, an
unverified one to confirm), fires the plugin event `actionPending`, and brings
the shell forward; the shell then takes the action and shows its chooser.

## Navigation rules

`navigation-policy-cases.json` is the shared table. `src/url-policy.ts` is
tested against it; `AppScreenPolicy.kt` and `AppScreenPolicy.swift` implement
the same rules:

- the main frame shows https only; `about:blank` / `about:srcdoc` stay (popups
  start there);
- the main frame shows only the app's own origin and the instance's Accounts
  origin. Any other https page (an external sign-in such as Google, which
  forbids OAuth in embedded webviews) opens in the browser view over the
  screen instead, and closing it comes back. Without a user gesture (a
  redirect, a script) this happens at most once every few seconds. A form
  POST to another origin is refused, with an offer to continue from the page
  in the browser view. A `target=_blank` / `window.open` to another origin
  goes to the browser view too, and popups keep their opener's origins;
- the Tauri shell's own hosts (`tauri.localhost`, `ipc.localhost`,
  `asset.localhost`) are refused;
- `http:` and other schemes (`tel:`, `mailto:`, `intent:` ...) go to the OS only
  after a confirmation that names the page's host, and only from the main frame;
- `javascript:`, `data:`, `file:`, `content:` and Takosumi Mobile's own
  `takosumi:` scheme are refused, as are URLs with user info. An Android
  `intent:` URI is judged by the scheme it would open (`#Intent;scheme=...`),
  and on Android it is also refused when Takosumi Mobile itself would handle it;
- without a user gesture, a page whose prompt was just declined cannot prompt
  again for five seconds;
- `target=_blank` and `window.open` (with a user gesture) open a new in-app
  screen stacked on the opener; `window.close()` closes it.

## Isolation

App screens are not Tauri webviews. They are plain `android.webkit.WebView` /
`WKWebView`s with their own configuration: no capability (the capabilities
apply to the shell's `main` window only and none has `remote`), no Tauri IPC,
no JavaScript interface, web message listener, script message handler, user
script or URL scheme handler. A page in an app screen therefore cannot call
plugin commands, files, the control plane or the instance's tokens.
`tests/app-screen-isolation.test.ts` and `tests/capabilities.rs` keep it so.

## Profile and lifetime

All app screens share one cookie and storage profile (Android's process-wide
WebView profile; iOS's default `WKWebsiteDataStore` and one process pool), so
signing in to Takosumi once on the web carries over to every app that uses
Takosumi OIDC. Do not give screens their own data directory, profile or
non-persistent store.

One app screen stays alive at a time (with its popups). Opening another app
closes the previous one's webview; it reopens by URL. Each app's last page is
remembered per (instance, account) on the device (`resume-url-cases.json`:
the app's own origin only, no user info, fragment or credential-named query
parameter), and reopening the app starts there. A deep link opens exactly the
page it names. Keeping several pages alive and switching between them waits
for device testing.

The host label shows the committed page only: on iOS it does not change while a
navigation is still loading, so a slow load of another site cannot borrow its
name. On Android, main-frame requests that are not https (POSTs and redirects,
which skip `shouldOverrideUrlLoading`) are refused before they load.

## Android specifics

- Each app screen is its own task (`FLAG_ACTIVITY_NEW_DOCUMENT`, document data
  `takosumi://app?key=...`) with a `TaskDescription` carrying the app's name and
  icon, so Recents lists apps separately. Back steps through history and then
  moves the task to the back, keeping the page.
- "ホーム画面に追加" pins a shortcut with `ShortcutManagerCompat.requestPinShortcut`
  (Android 8+, launcher permitting). The most used apps are kept as dynamic
  shortcuts (long-press menu). Shortcuts start `AppScreenActivity` directly, so
  an app opens without the Tauri shell starting.
- `AppScreenActivity` is not exported. External `takosumi://app?url=...` links
  reach the shell's deep-link handler, which resolves the URL to a known app.
- Files picked for `<input type=file>` are passed on only as content URIs from
  other apps' providers, never this app's own files.
- Camera, microphone and location requests from pages are refused for now
  (on iOS too).
