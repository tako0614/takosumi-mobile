# Whole Android App Web-home UI candidate

This is an external UI Automator driver for the actual Tauri Android App,
not an Activity hosting a substitute WebView. Source/local harness controls
and actual APK/driver compilation pass; the full whole-App UI flow remains
unqualified.

Run `bash native-tests/android/build-app.sh` first in a fresh CI checkout.
With the official API36 Google APIs x86_64 image, emulator/platform-tools,
KVM, Gradle8.14.3 and the same installed SDK/NDK available:

```bash
cargo build --locked --manifest-path native-tests/android/guard/Cargo.toml --target-dir src-tauri/target
python3 -B native-tests/android/run_web_home.py --out /absolute/new/private/evidence
```

The ordinary product APK has private package `com.takosumi.fixture.android`.
The driver and its instrumentation are `com.takosumi.fixture.webhome` and
`.test`; instrumentation targets the driver so an actual product force-stop
does not kill the test. UI Automator uses accessibility/OS input only. No
product callback, JS injection, cookie injection or native test UI opens a
connection. The ordinary App launcher returns to the connection picker after
Close removes a profile's Android Recents task.

The latest connection-add attempt also keeps a short diagnostic receipt: the
setText return time, the existing IME-presence/Back operation result, the same
readiness query's Save bounds, and click entry/return times. It records only
an attempt count, monotonic milliseconds, pixel bounds and booleans, without
input values or selectors. It adds no UI read, tap, retry, fallback or longer
wait. A setText return does not confirm the field value. Click wrapper times
do not prove actual input down/up delivery or JavaScript submit receipt.
This is a source candidate following run43279856761: the final image showed
text selection on Save, but could not establish why the first Web page was
not reached. Driver Java compilation has passed; native collection of this
connection-add trace remains pending because the next run stopped in setup.

Run43297320017 installed and launched the actual App, but failed the initial
form check before connection-add. Its final screenshot/hierarchy show the
OS's `System UI isn't responding` dialog over the form, as an earlier run
also did. The ANR onset and cause are unknown; no Save gesture ran in this
attempt. On failure only, the runner now rechecks the exact owned guest
before each `dumpsys activity lastanr` / `lastanr-traces` read. Each query has
one 10-second command allowance with the existing CLI retirement handling.
Raw output stays in private scratch; evidence keeps only the first 128KiB of
the summary and 512KiB of traces, with truncation/size/hash or closed error metadata.
An interrupted diagnostic retains scratch under the existing policy. Missing
or failed diagnostics preserve the original failure and cannot pass the UI.
The saved summary and newest trace file may concern different ANRs; their
process identities and timestamps must match before attributing a cause.
No dialog is dismissed, test retried, oracle relaxed or UI wait extended.

The two HTTPS sites and cookie/storage protocol are shared with the iOS
whole-App fixture. A private generated-project overlay trusts only this run's
public CA for the exact hosts `127.0.0.1` and `localhost`, without cleartext,
subdomains, global/user CA installation or SSL-error bypass. Both actual
compiled XML and packaged CA bytes are checked. The first native setup found
that apkanalyzer's decoded domain elements had no text; whether the original
compiled text was absent remained unproven. A separate aapt2 XML-tree decoder
now binds actual text to the same two domain leaves. Missing, conflicting,
foreign, duplicate or misplaced text is refused; expected hosts are never
used to fill omitted data. Structure/attribute/resource/CA checks remain.
The actual XML/CA checks and unchanged Cargo App bytes passed in
[CI43265225919](https://github.com/tako0614/takosumi-mobile/actions/runs/43265225919).
That run stopped during emulator ownership setup before App installation or UI.
The rebuild must retain the normal Cargo App library bytes. APK hash, installed base-APK hash, source
head/tree and Kit head bind the actual UI to the build; the normal SDK APK
is backed up privately and restored only after safe shutdown and unchanged
manifest/resource custody. Interrupted CLI descendants are not assumed idle:
the private project/CA/APK backup is retained, restoration is withheld and the
case fails in that situation.

One nonempty instrumentation case must complete connection→synthetic Web
login→home→management→cross-origin Notes→native Back/App Home, warm hide/show,
profile switch, real process force-stop/cold restoration and one-profile
data clear. Warm resume requires the same App PID and Web document; cold
resume requires both to change while drafts and independent Web sessions
persist. Clearing Work must reset its login/storage while Personal survives.
Server-side observations also require authenticated requests, actual Web
management/navigation and no instance cookie at the distinct editor host.

The existing product Linux guard source supervises the emulator without
compiling CEF/Tauri a second time. A fresh private AVD home, unique AVD name,
explicit serial/ports, stable process birth/SDK executable/guard ancestry and
ADB AVD-name readback are required before device operations. Success requires
normal emulator exit, no forced cleanup and the guard's kernel ECHILD proof.
The known guard PID remains the ancestry root even though its command line
forwards the same AVD arguments; it cannot stand in for an SDK child. The
ownership repair is covered by SDK-free controls and still needs native execution.
The existing shared adb service is outside that scope and is never killed.
The host must have no preexisting Android device; no foreign emulator is
stopped. A failed/incomplete drain retains scratch and cannot pass.

Only summary/CLI logs, closed server events, final hierarchy and screenshot
are uploaded. Product/driver APKs, generated debug key, synthetic cookie
values, private CA/server keys and AVD state stay outside evidence. Native
SDK/App UI success, physical-device behavior, release signing/updates, real
SSO, credentials, Push and other products remain separate qualifications.

`python3 -B native-tests/android/test_web_home.py` covers harness rejection
cases without Android SDK/UI execution. The existing APK policy controls
also continue to run. This workflow follows [external UI Automator testing](https://developer.android.com/training/testing/other-components/ui-automator),
[command-line instrumentation](https://developer.android.com/studio/test/command-line)
and [per-App/domain network security configuration](https://developer.android.com/privacy-and-security/security-config).
