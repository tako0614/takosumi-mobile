# App-screen native tests

These tests exercise the product's plain WebViews. They do not sign in to any
real service, use provider/DB/SSO credentials, or qualify distribution.

## Whole App iOS Simulator SDK build

On a disposable macOS checkout with Xcode, CocoaPods, Rust and Bun:

```bash
bun install --frozen-lockfile
bash native-tests/ios/build-simulator.sh
```

The script refuses an existing generated Apple project, uses the shared kit's
normal iOS generation/push wiring and builds the normal App with all iOS
plugins for the host's Simulator architecture. A separate bundle identifier
and `--no-sign` avoid installed App identity and Apple signing credentials.
Signing environment inputs are refused before generation/build because the
pinned CLI can import them before handling `--no-sign`.
It requires one nonempty Simulator `.app` with the exact fixture identifier
and Simulator platform, rejects a device IPA, and prints the executable digest.
CI runs this after the macOS desktop compile, serialized with the other desktop
SDK checks. The desktop matrix waits for the native job to finish but still
runs if that independent job fails; a native failure keeps the workflow red.
Quality failure or workflow cancellation prevents the matrix from running.
The SDK lane uses the standard `macos-26-intel` runner after hosted arm64
capacity prevented job acquisition. It compiles the host's x86_64 Simulator
target; Apple Silicon/iOS device runtime remains separate qualification.
This is an SDK compile check: it does not install or launch the App,
test WKWebsiteDataStore isolation, authenticate, or qualify APNs/distribution.
Simulator/device runtime checks remain separate from the portable Swift policy tests.

## Desktop GTK runtime fixture

On Linux with the Tauri GTK/WebKit SDK, Xvfb, xdotool, Python and OpenSSL, build
and run the executable fixture:

```bash
cp src-tauri/Cargo.lock src-tauri/plugins/app-screen/Cargo.lock
cargo build --manifest-path src-tauri/plugins/app-screen/Cargo.toml --features desktop-fixture --example desktop-profile-probe
python3 native-tests/desktop/run.py --binary src-tauri/plugins/app-screen/target/debug/examples/desktop-profile-probe --data /absolute/new/disposable-directory
```

Use a new absolute disposable data path. If using `CARGO_TARGET_DIR`, pass that
build's example binary instead. The runner creates a temporary loopback HTTPS
server and test-only certificate, then launches write and new-process readback
under isolated Xvfb/D-Bus sessions. Certificate trust is scoped to that test
view's context; no certificate is installed in the OS or product settings.
The fixture explicitly fails without its expected observations, including on
timeout. It uses the production raw-view
factory and synthetic HTML with https base URIs, checks cookies/localStorage
across two profiles/two sites and a separate legacy jar, absence of Tauri and
Wry messaging on GTK, same-screen reuse, retarget refusal and picker return.
All profiles first finish a real local HTTPS bootstrap document through the
production view factory. Synthetic loads begin only after those initial loads
finish; they do not cancel a pending factory request. Storage reads wait for an
exact native/DOM URL, DOM nonce, complete document and idle native load; an earlier page's
load event cannot start them. The regression deliberately finishes a previous
document first. A separate missing-document process must time out with no
storage observation. Each cookie/storage assertion runs once after readiness;
incorrect data is never retried. Native evaluation errors retain WebKit's error.
It also checks committed-origin display, pending/cancelled and iframe navigation,
native Back despite a page override, and native-keyboard-triggered popup creation,
profile/opener inheritance and script-close cleanup. Process readback concerns
settled writes after orderly teardown, not last-write durability or crash recovery.
This qualifies the GTK adapter's synthetic behavior; it is not Windows/macOS,
real IdP, file operations or distribution evidence.

Add `--spa-resume` with a fresh data directory to test `pushState` and
`replaceState`, immediate ordinary native Close, then an exact-route restore in
a new process without clicking an app link. `--failed-navigation` and
`--cancelled-navigation` each add a real native failure/cancellation before
Close; the next process must still restore the earlier successful SPA route.
These cases use production profile metadata, not a fixture-side saved URL.
GLib criticals fail the runner, including during teardown. Deferred URL snapshots
use weak view references so queued work does not keep a closed view alive.

## Full Linux App Web-home flow

This explicitly enabled fixture builds the entire App with its normal plugins
and setup, bundled production WebHome, real form submission and native IPC:

```bash
bun run build
cargo build --locked --manifest-path src-tauri/Cargo.toml --features native-web-home-fixture
python3 native-tests/desktop/web_home.py --binary src-tauri/target/debug/takosumi-mobile --data /absolute/new/disposable-directory
```

The feature is absent from normal/release builds. The runner uses a distinct
App identifier, private XDG data/config/cache/runtime directories and disposable
loopback HTTPS sites/certificate. No installed App data, real service or global
certificate settings are used. It drives connection input, mock Web login
(POST and HttpOnly cookie), home, a cross-site editor, native Home/Back/picker,
resume into the same focused native window, two isolated profiles, and native
Close. A new App process must restore both saved connections, both mock sessions
and each profile's editor storage. Cold opens must land directly in each
profile's cross-site editor without clicking its home-page link. A synthetic
credential callback before ordinary Close must never replace the safe start page.
The two App processes share one disposable
Xvfb/D-Bus desktop session. The runner observes App descendants and requires the
first App and its observed engine children to exit before starting a new App
PID; it does not destroy the desktop session during App teardown. DOM readiness
may be polled; data assertions run once. The actual views expose no native IPC.
Failure/timeout or a lingering engine process exits nonzero.

`--quick-close` minimizes the flow to one connection's acknowledged editor write,
immediate native Close, and new-process readback. CI runs it separately with a
fresh data directory, alongside the full two-profile flow.

The main cases use the normal App URL without `web-client=1`, proving the default
entry. A separate `--legacy-entry --quick-close` case explicitly selects the
preserved client, checks its desktop refusal and visible local return link,
then opens Web home. Its second App process uses the normal URL, so the return
choice must survive restart without another override. It does not read native credentials
or qualify the old client on mobile devices.

This covers the default Web path and ordinary window closure. An early diagnostic
using immediate App.exit lost a draft on restart; a delayed control and a later
run with stronger write acknowledgement both passed. The original separate
Xvfb/D-Bus session per App also lost storage during ordinary-close readback in
CI and a local minimized control. No unique cause or production durability fix
is established by these failures. The current runner checks an App restart
within a persistent desktop session, not desktop logout or storage fsync.
Immediate process termination, crashes, OS-by-OS cold restoration, real IdP,
other OS runtimes, files, notifications and distribution remain separate checks.

## Android

`resume-navigation-cases.json` exercises native success, provisional failure,
post-commit failure, stale/duplicate completion, SPA routes and unknown failing
URLs in Rust, Kotlin and Swift policy tests. The Kotlin callback adapter also
tests URL-to-generation binding for a late error from an older navigation.
These policy/JVM tests do not prove a device engine's callback order.

The Android fixture builds the actual `app-screen` plugin and its pinned Tauri
Android API without generating the full app or requiring Firebase configuration.
Set `TAURI_ANDROID_SOURCE` to `mobile/android` in the tauri crate selected by
`src-tauri/Cargo.lock`, use the installed Android SDK/JDK 17 and Gradle 8.14.3:

```bash
gradle --project-dir native-tests/android :app-screen:testDebugUnitTest :app-screen:assembleDebugAndroidTest
# With a test device/emulator attached:
gradle --project-dir native-tests/android :app-screen:connectedDebugAndroidTest
```

`ProfileIsolationTest` runs in an actual Android WebView. It writes synthetic
cookies and localStorage to two https origins using `loadDataWithBaseURL`, with
no network request to those sites. It checks two profiles at the same site,
two sites in one profile, recreated/evicted views, popup profile inheritance,
legacy shared-store preservation and rejection of a live view's profile change.
It also checks that those pages have no Tauri IPC. An engine without
`MULTI_PROFILE` fails the qualification test; it is not silently skipped.
The popup assertion calls the production registry factory; it does not trigger
the full `window.open`/WebView transport callback or an OAuth flow.

CI runs the JVM tests and instrumentation APK compile, then these engine tests
on a disposable Android emulator. Device/IdP login, file operations and updates
remain separate checks. Reports live under the ignored plugin `android/build/`.

The Swift policy suite runs on Linux CI via:

```bash
swift test --package-path src-tauri/plugins/app-screen/ios/PolicyTests
```

That suite verifies navigation and canonical profile names, not UIKit rendering
or iOS data-store isolation. WKWebView/device qualification remains necessary.
