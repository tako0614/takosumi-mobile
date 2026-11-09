# App-screen native tests

These tests exercise the product's plain WebViews. They do not sign in to any
real service, use provider/DB/SSO credentials, or qualify distribution.

## Public Web-app operation probe (experimental)

The explicit Linux-only `desktop-browser-probe` example uses the production
validated request and raw view factory with fresh Work/Personal profiles. It
opens only the reviewed Photopea or Excalidraw public root. No authentication,
account action, cloud save, provider/DB state or installed profile is used.
Normal App builds do not compile this probe.

Build once with the existing cached GTK SDK, then use a new owned data path:

```bash
cargo build --offline --manifest-path src-tauri/plugins/app-screen/Cargo.toml --features desktop-fixture --example desktop-browser-probe
python3 native-tests/desktop/browser_apps.py --binary /absolute/path/to/desktop-browser-probe --data /absolute/new/case --app excalidraw
python3 native-tests/desktop/browser_apps.py --binary /absolute/path/to/desktop-browser-probe --data /absolute/another/new/case --app photopea
```

This runner also needs existing Xvfb, D-Bus, xdotool and ImageMagick tools.
It scopes XDG profiles/cache/runtime and Downloads to the case, keeps the
desktop session alive through observed engine shutdown, and fingerprints the
binary before/after. It never changes HOME or installs OS infrastructure.
Before any public page opens, native startup requires GLib's Downloads path
to equal the disposable case's directory. It uses the existing WebKit default
download behavior without installing a destination override.
The selected file is a generated rectangle scene or red/blue SVG, opened
through the actual native file dialog. Excalidraw must export the imported
nonce-bearing rectangle with its keyboard edit; Photopea must export a PNG
with the actual fixture dimensions and decoded pixels. Data is checked once
after the real native download-finished event, without data retries.
The download must report the actual original Work view and the app's HTTPS
Blob origin. A per-document nonce is installed once immediately before export;
the native completion is followed by one read of that nonce and the exact
pre-export native URI. A same-origin reload or document replacement cannot
qualify as retaining the editor. Post-download HTTPS context and absent native
messaging are also required.
Native persistent directories and same-origin localStorage are checked in
both profiles. These are explicit checks, not proof of every app's IndexedDB,
authentication, file permission or service-worker behavior.

`--mode observe` is for UI discovery: it writes context/API observations and
screenshots and prints **OBSERVATION ONLY**. It never qualifies a file operation.
Default `operate` cannot accept an observer's receipt or a missing/unfinished/
foreign download. `python3 -B native-tests/desktop/test_browser_apps.py` checks
those false-success cases and the actual export verifier without opening sites.
The operation driver is experimental and needs actual native execution;
a selector/readiness failure must be diagnosed before attributing it to an
engine capability. A failure does not by itself select an engine or justify
growing bespoke WebView adapters instead of reusing an existing browser base.
API presence is not Web Push delivery, real IdP, background operation, another
OS or distribution qualification.

Initial 2026-11-05 probes at `1c82996` on WebKitGTK 2.52.3 reached both public editors and
checked their original persistent Work/Personal directories, same-origin
localStorage separation and absence of all three native messaging globals.
Both **file operations failed**: native Open imported the owned fixtures, but
the existing App navigation policy rejected `blob:` during export. Excalidraw
even displayed `File saved.` while the disposable Downloads directory stayed
empty. Photopea displayed the imported 128×96 red/blue image in `Save for web`,
then hit the same policy refusal. No completed native download or verified
export was obtained. That original scheme-only policy diagnostic preserved
the original allow decision. Probe load diagnostics retain public
URLs in private artifacts; this runner must not be used for real accounts.

The GTK adapter now makes an explicit native download decision for
HTTPS-origin Blob requests, rather than allowing them as page navigation.
It uses WebKit's existing default destination and original view/context; no
site API, download path or native dialog implementation is substituted.
With that repair, both public editors passed actual native Open and export:
Excalidraw saved the imported scene nonce and keyboard edit; Photopea saved
the actual 128×96 red/blue PNG. The strengthened checks also retained the
same live editor document and exact URI. The original failure cases remain
failures at their recorded source. Root processes exited successfully and
the existing observer waited for engine children; some WebKit children had
signal exits, so this is not a claim that every helper exited normally.
Native profile/popup/Back/new-process readback controls also passed separately.

Download cancellation/UI, authenticated downloads, active-download profile
deletion, Blob popup behavior and other platforms still need checks. Engine
selection, regular login/popup return, site permissions and Web Push delivery
remain open. Broadly allowing every Blob page navigation is not the repair.
The initial Chrome 149 comparison exposed PushManager and file-system picker
APIs and separated two isolated contexts; that initial operation was incomplete.
The later [standalone CEF candidate](cef-candidate/README.md) is a separate
existing-browser-foundation experiment. Its Photopea component verifies actual
PNG bytes and normal shutdown with explicit headless/CDP download settings.
Those Photopea headless/CDP results do not qualify default Save UI, regular
login, site permissions or Web Push delivery. The candidate README separately
describes its native picker/profile/prompt components and their limits. Those
CEF experiments are separate from this GTK repair and Tauri App integration.

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
The disposable generated target is restricted to the Simulator SDK/platform,
then regenerated with XcodeGen before the normal Tauri build, to address Xcode
26 selecting the device archive destination for an Intel Simulator build.
An unexpected template, existing platform override or wrong fixture identity
fails before build. The normal product's generated projects are not reused.
The normal kit Apple push writer runs again after regeneration; the development
entitlement must survive before compilation. This is wiring evidence, not push delivery.
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
Compiler caches are saved after non-cancelled SDK failures as well as success,
so a source repair can reuse completed compilation. Failed checks stay failed;
cached compiler files are not accepted App artifacts or runtime evidence.

### Android original-response comparison

The standalone [Gecko download fixture](android/gecko-download-fixture/README.md)
compares original cookie/Blob response streams, UUID context isolation/reopen and
cancel/close cleanup. Its Java-only toolchain and pinned engine are separate from
the product. It is a loopback HTTP capability experiment, not engine adoption,
product adapter completion, OS export, HTTPS or real IdP qualification.

### iOS download runtime qualification

At the boot-status wait deadline, the runner can observe that the same CLI has
already exited successfully. It accepts this only together with the latest
native terminal `Finished` record, and saves that paired readback. There is no
additional wait, reboot or retry. A live/nonzero CLI or a nonterminal/error log
still preserves the timeout. The original 2fd run did not reach XCTest; its
later cleanup inventory timeout remains a separate unresolved observation.
On failure after creation readback, the runner retains bounded `.log` tails
from that exact new device's recorded log directory before cleanup. It refuses
foreign paths, directory aliases, symlinks and multiply linked files; at most
four 64 KiB tails are copied through a held directory descriptor. Shared
CoreSimulator service logs and binary dumps are not collected. Missing logs
remain diagnostic absence and do not change boot deadlines or native verdicts.

The WKDownload candidate has policy cases for active Web-home/frame origins,
the explicit same-origin Blob exception, attachment disposition and safe
filename handling. The whole App SDK lane compiles its UIKit/WebKit wiring.
The first runtime fixture is `ios/download-fixture/run.py --out /absolute/new/evidence`.
It needs existing Xcode, XcodeGen, OpenSSL and an available iOS 17+ iPhone runtime
on macOS. It creates one disposable Simulator, trusts a new loopback HTTPS CA
only inside that device, and deletes that exact device after an identity readback.
It does not install SDKs/runtimes, alter host trust or use signing credentials.
The first cold boot has a 180-second readiness budget. Only distinct native
migration-plugin transitions near that deadline permit the same readiness
process to continue once for 120 seconds; elapsed counters alone do not.
Terminal `bootstatus` success is still required before trust or tests. A timeout
retires only the directly owned CLI, preserving the original failure separately
from termination errors. Interrupted commands retain scratch even if their
direct child reaps, because that alone cannot prove spawned build/test children
are gone. Unreaped commands also force failure; neither case authorizes group
signals or deleting a potentially live build directory.
Its small host compiles the product coordinator, navigator, policy and download
owner directly; only the unused Tauri shell signal is replaced with a refusal.
The existing desktop macOS CI job runs it after the whole App SDK build.

The actual XCTest uses visible links, a fixture session cookie, the native Save
prompt and the OS Save to Files UI. It reads the cookie/Blob exports from the
fixture App's Documents and requires their exact bytes. A second UUID profile
must receive the cookie-denied page. During the first cookie export, the host
removes the actual session while UIKit's activity is pending, observes that its
original file still exists with exact bytes, then the test completes Save to
Files. The final receipt requires this removal/retention observation and later
temporary cleanup; an omitted observation cannot pass. Decline, partial transfer Cancel and actual
session removal must remove only owned temporary files, keeping unrelated
sentinels. The server must observe both slow responses disconnect before their
last bytes; server shutdown cannot stand in for a WebKit cancel. Test output
must contain the actual method pass and one executed test with zero failures.
Logs, XCTest screenshots/hierarchy/result bundle and file receipts are retained;
ephemeral private keys remain outside the uploaded evidence. The Python harness
controls exercise real TLS/cookie and false-success checks, not native behavior.
The fixture is unqualified until its actual Simulator run passes. It is not the
whole Tauri UI, a physical device, a real IdP or an HTTP-auth qualification.

After this first case, cover navigation before consent, adapted/iPad export
presentation and return from an inactive application. The tiny exported files
already exercise completion during modal transitions, but that timing still
needs an actual Simulator result.
Use neither a replacement URLSession fetch nor a synthetic WKDownload delegate
result as the success oracle. Real IdP, device, HTTP-auth and distribution
qualification remain separate.

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
Each raw fixture generation now runs in a dedicated child-subreaper worker,
keeping its Xvfb/D-Bus session alive until observed engine children exit. It
reuses the full App runner's five-second shutdown observation and rejects a
reaped NetworkProcess failure before readback; unknown exit statuses remain
explicit. The missing-document negative also requires a completed generation
shutdown receipt, so its expected fixture status 1 cannot hide a helper failure.
This does not establish storage fsync or repair product durability.
An earlier raw run returned empty Personal localStorage on readback despite
a cookie and later correct disk contents. A guarded lifecycle control passed
on the unchanged binary, but the unique cause of that earlier loss is unproven.
This qualifies the GTK adapter's synthetic behavior; it is not Windows/macOS,
real IdP, file operations or distribution evidence.

Add `--spa-resume` with a fresh data directory to test `pushState` and
`replaceState`, immediate ordinary native Close, then an exact-route restore in
a new process without clicking an app link. `--failed-navigation` and
`--cancelled-navigation` each add a real native failure/cancellation before
Close; the next process must still restore the earlier successful SPA route.
`--pending-navigation` requires the exact next URI to remain loading at native
CloseRequested with no intervening failure/completion, then requires the same
saved SPA route on readback. The GTK adapter observes URI and native history
notifications synchronously, accepting a stable route only when both agree.
This covers replaceState when no history-list signal arrives. Active/requested
URI settlement and unqualified Close snapshots after a failure cannot overwrite
the last successful route. Later qualified history changes remain eligible,
including the URL of an earlier failed load, with the same credential filters.
These cases use production profile metadata, not a fixture-side saved URL.
GLib criticals fail the runner, including during teardown. Deferred URL snapshots
use weak view references so queued work does not keep a closed view alive.
The SPA close case also releases the raw profile cache while the UI loop is
alive and requires the native context's weak reference to clear within the same
five-second close bound. This detects the view/manager message-signal cycle
without relying on process exit. Context disposal and helper-process shutdown
remain separate checks. DOM probes start only after the native target URI has
settled; cookie/storage assertions still run once after the document nonce matches.
SPA readback asserts the saved successful route once before constructing its
view, then lets the production factory's initial navigation finish. It trusts
the disposable certificate on that context without reloading or loading the
expected route from the fixture. A native readiness timeout prints the actual
URI/loading state as well as the expected document; the five-second bound is
unchanged. This distinguishes saved-route failure from startup loading failure.

GTK stock error pages can emit a fresh Started/Committed/Finished cycle with
the failed display URI while loading an `about:blank` main resource. The adapter
preserves the saved route but invalidates active-document success for the
native default error classes; cancellation and the two native policy/plugin
exceptions retain their existing semantics. A later completed document must
have a current eligible HTTPS main resource. Its original resource URL need
not equal a same-document route, including when Back restores a cached page.

`--spa-resume --failed-navigation --error-page-close` waits for the actual stock
error document to commit and finish before ordinary Close, then checks the
saved successful route in a new process. This exposed the old behavior on GTK
2.52.3 even when immediate Close happened to pass. The same command passes
after the resource qualification fix, with temporary diagnostics removed.

Add `--error-page-back` to exercise native Back, require no new main-document
Response policy event, change the restored page's SPA route and check that
new route on restart. Add `--error-page-retry` instead to recover the synthetic
server after the error, call native Reload at the same failed URL and check
the now-successful route on restart. A file under the disposable fixture root
arms only this test server; it never writes product resume metadata. These two
controls require `--error-page-close` and are mutually exclusive.
`--cancelled-navigation --cancelled-spa` instead changes the surviving document's
SPA route to the very URL whose provisional load was cancelled, then checks
that route on restart after arming the same test server. Runtime results for
these recovery cases remain separate from the policy-model coverage.

The runner requires the writer's exact completion markers before readback;
an old or partial fixture that ignores a flag cannot qualify a case.
The raw `--profile-reset` case also requires its native reset completion marker
and uses the same bounded generation-shutdown worker, without a readback process.
`python3 -B native-tests/desktop/test_run.py` rejects four synthetic old/partial
recovery writers, an old reset writer and an abnormal native helper before readback. Those process controls
also reject a helper failure after the expected missing-document timeout. They
are not engine evidence.

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
Add `--reset-restart` with new data for three ordinary App generations: write
both connections, read them back and clear Work through the shared UI/native
IPC, then restart again and inspect the result. The final generation performs
no login, native clear, connection seeding or native resume-file write. It opens
Work from the saved picker, requires its root login page and an absent editor
draft, and requires Personal's retained session, draft and exact non-root route.
Personal's route is set through the real page History API before the clearer's
ordinary Close. Its exact pre-clear connection preferences are passed as a
fixture assertion expectation, never as replacement product state. The final
App compares the full saved value and both views' native persistent data/cache
directories to the original profiles, so a fresh replacement jar cannot pass
as a cleared profile. This checks ordinary restart after deletion, not crash/fsync
durability, OS logout or other OS runtimes.
In the three-generation case Work stays closed after native clear until the
final App opens it. Inspecting Work's editor earlier would itself create a new
successful last page, so expecting its root at the next restart would be wrong.
The independent warm case retains its immediate Work data inspections.

The independent single-generation `--profile-reset` case remains available.
The three-generation case requires the real clear completion marker before
starting the final App and a separate persisted-state marker from that App.
Old/partial Apps that ignore either mode cannot qualify a case. Four synthetic
process controls cover missing markers, missing profile identity and the successful protocol; they
are not native-engine evidence. Per-document, process and shutdown bounds and
one-shot data assertions remain unchanged.
The dedicated Linux session worker adopts orphaned App children and records
their exit status. An adopted NetworkProcess error or signal refuses readback;
renderer termination policy is recorded separately. A NetworkProcess already
reaped by an intermediate parent has an unknown exit status, reported explicitly;
its disappearance is lifecycle evidence, not proof of graceful storage flush.
Only observed matching PID start identities are reaped. A timeout prints bounded native thread wait names,
without page URLs or stored data. The five-second shutdown bound is unchanged.
`python3 native-tests/desktop/test_web_home.py` checks normal, abnormal and
signalled synthetic helpers, delayed network-role detection, parent-reaped
unknown status and a mismatched PID identity without starting a browser.

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

`--profile-reset` uses a fresh single App process to exercise the same shared-UI
clear path after the two profiles' mock login and editor writes. It checks the
native live-view refusal, completed receipt, cleared Work and preserved Personal
without depending on an earlier App shutdown/readback. The normal full flow
retains the clear assertions after cold restoration too. The runner requires
the dedicated native-reset success marker, so an older binary that ignores the
new mode cannot qualify it. `--quick-close`/`--legacy-entry` cannot be combined
with this mode. CI runs reset and ordinary restoration as independent steps
after one App build: a runtime failure keeps the job red while allowing the
other case to return its own evidence. A failed/cancelled build never supplies
an App acceptance result.

## Android

`AppScreenMediaConsentTest` checks the camera/microphone consent state machine,
including OS denial, stale results and document/profile changes. The actual
`MediaPermissionActivityTest` uses an intercepted HTTPS fixture in the production
WebView and Activity, without substituting the WebChromeClient. It requires the
origin-named site prompt, observes a still-pending `getUserMedia` request before
site denial, then opens a new document and accepts the site prompt. The real
Android permission UI must appear while the request is still pending; OS denial
must reject it and return to the same resumed screen. The same scenario then
explicitly allows the OS prompt and requires one live, enabled native audio
track, no video track, and an inactive stream with ended tracks after stopping.
A fresh profile at the same origin must still show the site prompt despite the
app-wide OS grant, and its denial must reject the request. Missing dialogs,
skipped tests, early rejection or early grant cannot qualify these paths.

The disposable CI AVD explicitly enables guest audio input. The first
`-no-audio` run reached OS allow but rejected capture with `NotReadableError`;
its cause is not established. The next fixture uses the default audio backend
without that flag. [QEMU2 forwards `-no-audio` to its disabled driver](https://android.googlesource.com/platform/external/qemu/+/emu-master-dev/android-qemu2-glue/main.cpp#1950);
zero-filled input in another backend path does not prove WebView can start.
Host microphone input is never opted in, and the
[documented `adb emu avd hostmicoff` command](https://developer.android.com/studio/releases/emulator)
must acknowledge `OK` before testing. That acknowledgment is not an independent
host-mic state readback. Failures retain bounded audio logcat, the DOM exception
message and input-device types (no device identifiers or labels). The native
result remains required: no synthetic WebView grant is used. The default
backend run still returned `NotReadableError` and logged a failed `pa` driver
initialization. The next CI fixture starts its own PulseAudio server on a
private UNIX socket with only a null sink and its monitor source. It loads no
hardware discovery modules and reads back the exact sink/source/module list
before starting the emulator. Host input remains disabled. Cleanup targets only
the owned server and emulator; no shared audio service is changed. Cleanup
captures each child PID and kernel birth, binds signals with a Linux pidfd,
and bounds TERM/KILL waits to three/two seconds. Four real-child controls cover
normal exit, forced exit, wrong identity refusal and an already-exited child.
Unverified identity or failed cleanup makes the job fail; it never falls back
to signaling a raw PID. The EXIT trap explicitly preserves the original failure
or reports a cleanup failure. The PA artifact is capped at 64 KiB. This is
a targeted backend experiment, not proof that PA initialization caused the
browser failure. The probe checks stream lifecycle without recording or
inspecting samples.
It does not qualify physical microphone quality, camera capture, background
capture, cross-origin iframe permission or other Android/OEM versions.

The plugin manifest also declares `MODIFY_AUDIO_SETTINGS`, Android's normal
audio-routing permission. The matching WebView 133 code refuses low-latency
input-device selection without it, even when `RECORD_AUDIO` is granted. The
native probe checks the installed permission before audio initialization and
still requires the real site/OS consent and stream lifecycle. The fresh test
install/process matters because Chromium caches the normal permission. This
fix does not by itself qualify the audio backend or physical capture.

The profile-reset instrumentation requires both `MULTI_PROFILE` and
`DELETE_BROWSING_DATA`; unsupported capabilities fail instead of skipping tests.

The instrumentation APK targets SDK 36, matching the generated product App's
`src-tauri/gen/android/app/build.gradle.kts`. Earlier native results used the
library's implicit target SDK 28 even on the API 36 emulator; retain that
limitation when interpreting their OS permission and storage behavior.
The media and file-selection cases assert the actual runtime target SDK.

`FileSelectionActivityTest` creates one uniquely named small text file in the
disposable emulator's MediaStore Downloads collection, opens a real WebView
file input with a pointer gesture, and selects that exact file in DocumentsUI.
It requires the original document to receive the exact name, byte length,
MIME type and `File.text()` contents, then resume the same profile. It deletes
only the URI it created. No broad storage grant, provider substitution,
WebChromeClient replacement or simulated picker result is used. The DocumentsUI
selectors cover the default English CI AVD; other picker apps, OEMs, locales,
multiple-file selection and recreation while a picker is open remain separate.
The initial API35 emulator used WebView124 and passed profile isolation but
failed the two deletion preconditions. The next CI candidate selects the
official API36 Google APIs x86_64 image and logs its revision and actual WebView
package. A newer image is not itself acceptance evidence: the same feature and
data assertions must pass. The App's Android minimum and runtime capability
guards remain unchanged; no default/shared-jar deletion fallback is used.

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

## Profile data reset candidate

The shared WebHome confirmation has a separate actual-browser component test.
From the product root on Linux, with modern Node (22.12+), Python with pidfd
support, the existing dependencies and an installed Chrome:

```bash
bunx tsc --noEmit -p native-tests/browser/tsconfig.json
PYTHONDONTWRITEBYTECODE=1 python3 native-tests/browser/test_guard.py
node --experimental-strip-types native-tests/browser/run.ts /absolute/new/artifact-directory
```

It starts its own loopback-only Vite listener and fresh headless Chrome profile,
and closes both. `CHROME_BIN` may select the test Chrome executable. It never
uses an existing browser or login profile. The synthetic https connection labels
are never opened. The browser's HTTP(S) proxy is configured for a loopback
endpoint with loopback bypass. Chrome runs below a dedicated session/subreaper. Detached helper
identities are tracked with pidfds and reaped before deleting the new profile;
if cleanup cannot be confirmed within the bound, the private profile is retained
and the command fails. The synthetic guard controls include a detached orphan
and cancellation, late forks, observation failure, PID generation replacement,
stale discovery from another ancestry and failed ancestry reads. They preserve
a separate session. The final pass receipt is
written only after browser and Vite cleanup succeed.
The actual shared component and clear-request adapter render three synthetic
connections. It checks cancel/confirmation locks, pending state, false receipts,
native refusal, completed status and preservation of settings, then confirms
that removing a setting does not invoke another deletion. Desktop and mobile
width screenshots also reject horizontal overflow. Native results are simulated:
this proves the UI/request path, not engine deletion or a mobile/desktop runtime.
The receipt includes browser/Node versions and before/after source hashes. PNGs
live in the new artifact directory. CI runs this on Linux.

The reset candidate adds a separate, explicitly confirmed action to the
shared Web home. Removing a connection remains a preference edit. A clear
request is bound to a canonical isolated UUID and its exact connection key;
the legacy shared profile is refused. Engines advertise the action through a
separate capability and the shell requires an explicit completed receipt.
The desktop candidate grants this only to the bundled `main` shell and exposes
the same confirmation as mobile. macOS requires the UUID-store API (14+).
Its native SDK/runtime qualification remains pending; source availability alone
does not qualify data deletion.

Android uses the selected profile's `WebStorage` and the
`WebStorageCompat.deleteBrowsingData` completion callback, gated by both
`MULTI_PROFILE` and `DELETE_BROWSING_DATA`. It does not clear process-default
storage or use `ProfileStore.deleteProfile` as a completion receipt. Live views
block deletion. A latch blocks opening the profile during deletion. Committed
profile generations also invalidate stale Activity/saved-instance URLs and
late callbacks after an inactive view was evicted. New resume metadata is
indexed by profile; old opaque v1 hashes cannot be safely assigned for physical
removal, so old-generation restoration is disabled without deleting another
profile's preferences. This resets restoration, not forensic disk erasure.

iOS removes all website data in the selected UUID store and waits for WebKit's
completion callback. Any live same-profile session blocks the operation and
configuration creation is refused while clearing. Resume metadata is removed
for every scope with the exact cleared key. Other profiles, legacy native
credentials, connection settings and files saved outside site storage are not
selected. Completion does not claim fsync, crash or downgrade durability.

The shared `clear-profile-cases.json` is consumed by Rust/Kotlin/Swift policy
tests. The Android instrumentation case checks live-view refusal, pending-open
refusal, two-site cookie/storage removal and preservation of a second profile
and the shared store. `ProfileResetActivityTest` exercises an actual stopped
Activity whose WebView is evicted, completes a profile reset, then resumes the
same Activity. It also recreates an Activity with a deliberately pre-reset
saved URL after dropping the live view; assertions require both controls to
have run. A lifecycle probe is armed before each resume/recreation, attaches on
the same UI turn after the new view is made, freezes its first native page-start
callback and restores the production client before forwarding it. A later root
load cannot replace an observed wrong URL. The document-load wrapper is also
removed after its awaited completion; neither wrapper qualifies general error
handling.
The latter saved-state input is synthetic; this is not an OS process-kill test.

The Android JVM suite passes (17 tests), and the instrumentation APK compiles
locally. Rust passes 9 policy/unit tests and 2 capability tests. The new engine
and Activity tests have not run on an emulator/device; there is no local KVM
capability. Swift/native SDK checks remain separate. CI requires the two reset
cases by class and test name, with zero
failures/errors/skips. Portable TypeScript checks do not qualify native data
deletion or UI behavior.

Desktop uses an invisible blank raw view with the same cached profile context
as normal screens. Navigation/popups and messaging are disabled. GTK validates
the data/cache directory before `WebsiteDataManager.clear`; macOS validates the
UUID store before WebKit removal; Windows waits for `ClearBrowsingDataAll`'s
HRESULT callback. All native objects stay on the UI thread. The helper and
profile latch remain until native completion, background removal of the known
resume preference, and native helper teardown. Other profile files are not
selected. Opens (including popups) and duplicate clears are refused meanwhile.
Session-end teardown invalidates operations, drains outside the mutable borrow,
and blocks new work; native construction rechecks the operation after any OS
event pumping. Late callbacks cannot remove a newer operation's preference.

The Linux plugin passes 17 unit/policy tests plus 2 capability tests and the
fixture compiles. `run.py --profile-reset` adds live/shared/pending refusal,
pre-clear reads of both target sites, one native clear, then one set of data
observations for target emptiness and Personal/shared preservation. CI includes
this new case. Locally its initial run and a failure-diagnostic control timed
out before clearing, after observations 0/1. The diagnostic control observed
two WebKit NetworkProcess persistent threads in `jbd2_log_wait_commit`; that
does not establish the sole cause or qualify data deletion. Its timeout and
data assertions remain unchanged. The bounded timeout snapshot reads only the
owned session's process names/identities/waits, with no argv or page payload;
a subprocess control verifies another session is untouched and unobserved.
The full bundled App fixture also clears Work through the real shared
confirmation/IPC after restart readback. It first checks refusal while Work is
open, then closes Work and waits for the completed receipt. Reopening must land
at its root logged out, with no editor draft at the second site. A live Personal
profile must retain its draft and its HttpOnly session on a fresh home request;
connection settings must be unchanged. These new assertions run once after DOM
readiness, alongside the existing cold-restoration flow.
Two local warm-mode attempts on WebKitGTK 2.52.3 remain overall red. The normal
candidate passed live refusal but remained pending at the eight-second UI check.
A separate diagnostic build observed native completion, metadata removal, helper
teardown and reply, then passed the real shared-UI clear and preservation assertions.
Its observed NetworkProcess shutdown exceeded the unchanged five-second bound;
journal/bio waits and a test-desktop cleanup failure were recorded. That is
component evidence, not a normal-candidate or whole-run pass, and does not prove
the first delay's unique cause. Temporary observation code was removed.
macOS/Windows compilation, native clear/error/teardown runtime across platforms
and restart durability remain unqualified for this candidate.
