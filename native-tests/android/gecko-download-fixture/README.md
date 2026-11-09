# Android original-response capability comparison

This standalone fixture compares GeckoView `159.0.20261102144711` with the
current Android WebView DownloadListener boundary. It is not linked into the
product and does not select a replacement engine. Native execution is required;
source inspection or the portable App tests cannot qualify it.

One visible GeckoView uses fresh Work/Personal UUID context IDs. Instrumentation
taps the actual page sign-in and download links. The engine's
`ContentDelegate.onExternalResponse` hands its original body to one worker;
there is no second HTTP client, cookie export, response replacement or JS bridge.
The test compares exact cookie and Blob bytes, verifies a different context is
denied and the same context remains signed in after reopening, then cancels two
real slow streams (explicit Cancel and closing Work). Partial files are removed
only after stream/output closure. The server must observe both incomplete
responses disconnected before server teardown. Personal's unsigned state and
an unrelated sentinel must remain intact.

Native controls retain their exact visible label casing, including in Android's
accessibility tree. The test first requires every control to be visible and
enabled; it still injects actual pointer input. A failed wait includes a bounded
accessibility snapshot captured before Activity cleanup, without relaxing text
matching or invoking button handlers directly.

If an original response does not arrive, the same 20-second wait now reports
the visible accessibility tree and the last 32 native navigation/response events.
Events record only fixture URI classes, decisions and numeric error codes, not
URLs, cookies or bodies. The Blob button updates its own page text when its
actual click handler starts, dispatches the anchor click, or throws. These
observations distinguish missed input, page script failure, navigation refusal
and missing engine response; none is accepted in place of original bytes.

The server binds only `127.0.0.1` on an ephemeral port. Its synthetic HTTP cookie
is not a real credential. This first transport experiment does **not** qualify
HTTPS/Secure cookies, external IdP login, OS Save to Files, background recovery,
product permissions/popup/upload, or completed profile deletion. Gecko's
`clearDataForSessionContext` dispatches without a completion result; calling it
alone cannot replace the product's completed-reset contract.

## Anonymous Photopea Open and OS export

The separate `WebAppOperationTest` opens the real public Photopea page in two
fresh contexts. A new owned Downloads PNG contains a 128x96 red/blue image.
Each fresh profile enters through the visible `Start using Photopea` button if
the ordinary landing page is shown. There is at most one real tap per profile,
and the same 60-second wait still requires the editor's File menu before Open.
When the first entry button is below the actual GeckoView viewport, one native
upward swipe reveals it. Offscreen accessibility bounds authorize that scroll
only: the tap still requires a visible action wholly inside the viewport.
The retained event trace records scroll/tap bounds; no script click or page
replacement is used. All native taps must fit inside the active window.
The main-thread viewport read waits at most one second within the remaining
entry deadline. A stalled read fails and cancels its queued callback; a late
read cannot inject input.
Actual native keyboard input opens Photopea's file prompt; the engine prompt
uses Android's real Open Document picker and returns its URI to the same page.
After switching to Personal and back without reloading Work, the test invokes
Photopea's Save for web and receives the original HTTPS Blob response. Native
confirmation and Create Document select a fresh owned PNG. A single worker
copies the original stream to that provider URI, with no URL refetch or cookie
export. The test reads the OS output once, checks every decoded pixel and the
native navigation generation/URI, and keeps Personal's separate editor.

This small fixture uses Gecko's existing file prompt/response interfaces,
the same boundaries exposed by Android Components. It does not build or adopt
Android Components or a new product browser. It covers one PNG, the English
DocumentsUI, anonymous Photopea and foreground operation only. Real app input,
selectors, HTTPS response and provider output must execute before any native
success is claimed. An editor load or a successful Java compile is insufficient.
Missing native controls stay failures with a bounded accessibility snapshot
and screenshot; no page/picker response is substituted. The copied PNG evidence
comes from the actual OS output only after pixel verification. Cleanup waits
for the owned writer and stream closure before removing only returned owned
documents and the test's input row. A closure failure retains those files.
Evidence uses AndroidX `PlatformTestStorage` and the connected runner's
additional-test-output collection. CI requires the exported PNG there on a
successful run, and uploads the bounded hierarchy/screenshot when available.
It does not depend on app external files surviving Gradle's APK retirement.
The EXIT trap records the captured emulator PID/birth, original step status,
owned-stop result and same-shell child wait. That receipt concerns the direct
emulator child, not exhaustion of descendants or shared adb services.

The existing loopback test remains unchanged. The CI requires both exact test
methods, two executed cases and zero failure/error/skip. This does not qualify
product integration, full profile storage/reset, IdP, media/popup/notifications,
background recovery or other Web apps. Excalidraw scene Open/edit/export is the
next separate operation comparison after this first actual public app result.

## Separate build and owned runtime

The pinned [Mozilla POM](https://maven.mozilla.org/maven2/org/mozilla/geckoview/geckoview/159.0.20261102144711/geckoview-159.0.20261102144711.pom)
identifies upstream revision `7b34a03a910d0385299f4a9f27aa585a786862d8`.
Its AAR metadata requires API 37.2; transitive AndroidX Core 1.19.1 also requires
newer AGP. This Java-only project therefore uses AGP 9.1.1, Gradle 9.3.1 and the
`platforms;android-37.2` SDK package, independently of the product's API 36 /
AGP 8.11 / Kotlin 1.9 build. The runtime fixture still targets API 36, on a fresh
API 36 x86_64 emulator. No AAR metadata constraint is suppressed.

The CI `android-download-candidate` job compiles the exact pin, creates one owned
AVD, refuses pre-existing devices and checks two non-skipped instrumentation
methods. It stops only the emulator PID/birth captured from its own launch.
An incomplete/failed run stays RED. It neither retries an old failure nor changes
the existing product native job. To compile on a prepared isolated host:

```bash
gradle --no-daemon --project-dir native-tests/android/gecko-download-fixture assembleDebug assembleDebugAndroidTest
```

Use Gradle 9.3.1 and the SDK above. Do not point connected tests at a personal
device; the CI step supplies the disposable runtime and ownership checks.
