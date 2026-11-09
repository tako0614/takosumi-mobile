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

The server binds only `127.0.0.1` on an ephemeral port. Its synthetic HTTP cookie
is not a real credential. This first transport experiment does **not** qualify
HTTPS/Secure cookies, external IdP login, OS Save to Files, background recovery,
product permissions/popup/upload, or completed profile deletion. Gecko's
`clearDataForSessionContext` dispatches without a completion result; calling it
alone cannot replace the product's completed-reset contract.

## Separate build and owned runtime

The pinned [Mozilla POM](https://maven.mozilla.org/maven2/org/mozilla/geckoview/geckoview/159.0.20261102144711/geckoview-159.0.20261102144711.pom)
identifies upstream revision `7b34a03a910d0385299f4a9f27aa585a786862d8`.
Its AAR metadata requires API 37.2; transitive AndroidX Core 1.19.1 also requires
newer AGP. This Java-only project therefore uses AGP 9.1.1, Gradle 9.3.1 and the
`platforms;android-37.2` SDK package, independently of the product's API 36 /
AGP 8.11 / Kotlin 1.9 build. The runtime fixture still targets API 36, on a fresh
API 36 x86_64 emulator. No AAR metadata constraint is suppressed.

The CI `android-download-candidate` job compiles the exact pin, creates one owned
AVD, refuses pre-existing devices and checks one non-skipped instrumentation
method. It stops only the emulator PID/birth captured from its own launch.
An incomplete/failed run stays RED. It neither retries an old failure nor changes
the existing product native job. To compile on a prepared isolated host:

```bash
gradle --no-daemon --project-dir native-tests/android/gecko-download-fixture assembleDebug assembleDebugAndroidTest
```

Use Gradle 9.3.1 and the SDK above. Do not point connected tests at a personal
device; the CI step supplies the disposable runtime and ownership checks.
