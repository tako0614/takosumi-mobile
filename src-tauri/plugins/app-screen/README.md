# Takosumi mobile app screen plugin

This product-owned Tauri plugin opens an installed app (or a URL the person
added on the device) in its own native webview: `WKWebView` on iOS and
`android.webkit.WebView` on Android. The webview is separate from the Tauri
shell's own webview and sits under a thin native bar:

- the current host, always visible (there is no address field, so a page
  cannot pretend to be another site);
- back, reload and close;
- a menu with share, open in the browser, switch apps and, on Android, add to
  the home screen.

Design: `takos-control/docs/platform/takosumi-mobile-browser.md` (TASK-0119
U4, step 3). Push delivery (step 4) and the iOS widget and App Intents
(step 5) are not part of this plugin yet; the webview factories mark where the
document-start Push API bridge will be added.

## Commands

| Command | Payload | Result |
| --- | --- | --- |
| `capabilities` | none | `{ pinShortcut }` (Android launchers that can pin) |
| `open` | `{ key, url, title, iconPng?, profile: "shared" }` | opens or returns to the screen |
| `pin_shortcut` | same as `open` | `{ requested }`; always `false` on iOS |

`key` is the screen's stable identity (`app:<capsuleId>:<interfaceId>`,
`custom:<id>` or `origin:<origin>`). `iconPng` is a base64 PNG the shell drew
from the Interface `display.icon` (SVG and glyphs are rasterized in the shell;
the native side only decodes PNG). Rust, Kotlin and Swift each re-check that
the URL is https without user info and that `profile` is `shared`.

## Navigation rules

`navigation-policy-cases.json` is the shared table. `src/url-policy.ts` is
tested against it; `AppScreenPolicy.kt` and `AppScreenPolicy.swift` implement
the same rules:

- the main frame shows https only; `about:blank` / `about:srcdoc` stay (popups
  start there);
- `http:` and other schemes (`tel:`, `mailto:`, `intent:` ...) go to the OS only
  after a confirmation that names the page's host, and only from the main frame;
- `javascript:`, `data:`, `file:`, `content:` and Takosumi Mobile's own
  `takosumi:` scheme are refused, as are URLs with user info;
- `target=_blank` and `window.open` (with a user gesture) open a new in-app
  screen stacked on the opener; `window.close()` closes it.

## Profile and lifetime

All app screens share one cookie and storage profile (Android's process-wide
WebView profile; iOS's default `WKWebsiteDataStore` and one process pool), so
signing in to Takosumi once on the web carries over to every app that uses
Takosumi OIDC. Do not give screens their own data directory, profile or
non-persistent store.

Up to four screens stay alive when the person switches between apps; the least
recently used one is closed first and reloads its last URL when it returns.

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
- Camera, microphone and location requests from pages are refused for now.
