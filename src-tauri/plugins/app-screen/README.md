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
| `open_web` | `{ url }` | `{ result: "browser" }`: the phone's browser, never an app claiming the link (Android `CATEGORY_APP_BROWSER` selector; iOS cannot stop a universal link) |
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

`navigation-policy-cases.json` (and `resume-url-cases.json`) are the shared
tables, and all three implementations are run against them:
`tests/url-policy.test.ts` (`src/url-policy.ts`), `AppScreenPolicyTest`
(`AppScreenPolicy.kt` with Android's own `Uri`, through Robolectric:
`./gradlew :tauri-plugin-app-screen:testDebugUnitTest` in a `tauri android
init` project) and `ios/PolicyTests` (`AppScreenPolicy.swift`, a package of
its own that builds without UIKit: `swift test --package-path
src-tauri/plugins/app-screen/ios/PolicyTests`, on macOS or Linux):

- the main frame shows https only; `about:blank` / `about:srcdoc` stay (popups
  start there);
- the main frame shows the app's own origin. On the Takosumi origins (the
  instance's Accounts, i.e. the OIDC issuer, and the instance itself, where
  the dashboard, `/api/v1` and the `takosumi_session` cookie live) it shows
  only the pages an app's Takosumi sign-in passes through:
  `/oauth/authorize` (authorization and consent), `/sign-in` and
  `/sign-in/callback` (the sign-in screen it redirects to) and
  `/oauth/upstream/authorize` / `/oauth/upstream/callback`, compared exactly
  on the percent-encoded path. Every other page there (`/`, `/apps`,
  `/account`, `/api/v1/*` ...) goes to the browser view, and is refused in
  subframes, so the management screens never run inside an app screen. The
  sign-in screen is the dashboard's single-page app, so URL changes made with
  `history.pushState` are checked too: a move to a dashboard page goes back
  and opens that page in the browser view. An app screen is never opened on
  such a page (tiles, deep links, shortcuts open it in the browser view);
- any other https page (an external sign-in such as Google, which forbids
  OAuth in embedded webviews) opens in the browser view over the screen
  instead, and closing it comes back. Without a user gesture (a redirect, a
  script) this happens at most once every few seconds. A form POST to
  another origin is refused, with an offer to continue from the page in the
  browser view. A `target=_blank` / `window.open` to another origin goes to
  the browser view too, and popups keep their opener's origins;
- the Tauri shell's own hosts (`tauri.localhost`, `ipc.localhost`,
  `asset.localhost`, any other `*.localhost`) are refused in every frame and
  over http as well as https;
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

## Known limits

These follow from the platforms and from how trust is decided; they are not
bugs the plugin can close.

- **iOS custom schemes can be claimed by any app.** On iOS another installed
  app may register the same custom scheme as a declared native app, and iOS
  gives no way to pin the target (Android pins it with `setPackage`). Only the
  instance URL travels in those links, never a credential, but a fake app
  could imitate the real one. Universal links (`universalLinksOnly`) are the
  safer declaration.
- **"Verified" is decided by the instance.** A native app is verified when the
  instance's operator list of trusted native clients says so
  (`TAKOSUMI_ACCOUNTS_TRUSTED_NATIVE_CLIENTS`). A malicious or careless
  instance can mark any scheme or package that is not on Takosumi's refused
  list as verified, and Takosumi Mobile then opens it without asking. Signing
  in to an instance means trusting its operator with this.
- **One web profile for every app screen.** All app screens share one cookie
  and storage profile, across instances and accounts; only the remembered
  last pages and recent apps are kept per (instance, account). Signing out of
  a Takosumi instance in the shell does not sign app screens out of the
  apps' own sessions, and a sign-in made in an app screen for one instance's
  account is visible to every app screen on the device.
- **Browser-view sign-ins stay in the browser.** A sign-in completed in the
  browser view (an external IdP such as Google, or a Takosumi page there)
  lives in the browser's cookie jar, not in the app screen's profile, so the
  app screen itself may still be signed out afterwards.
- **iOS cannot force Safari.** 「Safariで開く」 / 「ブラウザで開く」 on iOS opens
  the URL with the system, and an app that claims it as a universal link
  takes it.

## Profile and lifetime

All app screens share one cookie and storage profile (Android's process-wide
WebView profile; iOS's default `WKWebsiteDataStore` and one process pool), so
signing in to Takosumi once on the web carries over to every app that uses
Takosumi OIDC. Do not give screens their own data directory, profile or
non-persistent store.

One app screen stays alive at a time (with its popups). Opening another app
closes the previous one's webview; it reopens by URL. Each app's last page is
remembered per (instance, account; the key is a SHA-256 hash) on the device (`resume-url-cases.json`:
the app's own origin only, no user info, fragment or credential-named query
parameter), and reopening the app starts there. Only pages reached in an app
opened as itself (a tile, a recent app, a shortcut, the chooser) are
remembered; a deep link opens exactly the page it names and never becomes the
last page. Sharing or handing the current page to the browser (view) removes
credential-named query parameters and a fragment holding parameters
(`resume-url-cases.json` `shareCases`), so a spent OAuth `code` is not passed
on. Download prompts are limited to one every few seconds. Keeping several pages alive and switching between them waits
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
- Task restore and shortcuts keep a declared native app only as its label,
  never its links: 「{name}で開く」 from such a screen asks the shell, which
  reads the current declaration. After the system recreates a screen, or its
  webview was closed for another app, it reloads only a page that passes the
  resume rule (no spent OAuth callback, no Accounts page).
- `AppScreenActivity` is not exported. External `takosumi://app?url=...` links
  reach the shell's deep-link handler, which resolves the URL to a known app.
- Files picked for `<input type=file>` are passed on only as content URIs from
  other apps' providers, never this app's own files.
- Camera, microphone and location requests from pages are refused for now
  (on iOS too).
