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

Run43305494368 reached Work sign-in, management, Notes/draft and native Back/Home,
then the first saved-Work lookup failed. Its screenshot and accessibility tree
retain the enabled, visible connection button; WebView combines its name,
origin and hint into one label, with no separate `Work` node. The driver now
finds package-scoped, enabled/clickable visible buttons and matches the exact
name and complete origin in text or description. Only the known `開く →` or
`前の画面へ戻る →` hint may follow. One unique button is clicked once; multiple
matches fail and the existing 15-second lookup deadline remains.
The actual matcher also runs in the Android-SDK-free Python controls using
an existing JDK17+: merged/newline labels pass, while name/port prefixes,
other origins, URL suffixes, deletion buttons and unknown trailing labels
fail. Quality explicitly prepares JDK17. This does not qualify the Android
driver's new selector until its native run.

Run43311887068 compiled the new driver and installed the actual App, but
stopped earlier at the first connection Save. Its one IME query reported no
keyboard; the final screenshot and accessibility tree showed Gboard with
Save outside the viewport. No Save click ran. Late keyboard visibility is
consistent with this sequence, while a configured-IME/AX-owner mismatch is
not excluded. The connection Save wait now rechecks the same configured IME
package within its existing 15-second readiness loop. It requests Back only
on a positive match and at most once, including when Back returns false;
then it continues requiring the actual Save control and clicks once. Other
control waits retain their existing behavior. The new IME handling and saved
connection selector remain unqualified by this run.

Run43317859224 passed Work Web login/management, Notes/draft, saved-Work
selection and background return with the same App PID/document. It then
stopped before Personal input: the saved Work row pushed the form below the
viewport, with no keyboard shown and Save outside the accessibility tree.
Only the initial form wait may now scroll once, when Save is absent or has
empty bounds, the add heading is visible and exactly one owned scrollable
WebView is enabled/visible. A half-viewport `scroll(Direction.DOWN, 0.5f)`
uses the real gesture API within the same 15-second readiness bound. Its
return value is not an acceptance result; the actual enabled/visible Save
must still appear. Other waits, input and Save counts are unchanged. The
scroll path and Personal/cold/reset stages remain natively unqualified.
The pinned UI Automator2.3.0 API is described in the
[UiObject2 reference](https://developer.android.com/reference/androidx/test/uiautomator/UiObject2#scroll(androidx.test.uiautomator.Direction,float)).

The latest connection-add attempt also keeps a short diagnostic receipt: the
setText return time, the existing IME-presence/Back operation result, the same
readiness query's Save bounds, and click entry/return times. It records only
an attempt count, monotonic milliseconds, pixel bounds and booleans, without
input values or selectors. The trace itself adds no UI read, tap, retry,
fallback or longer wait; the bounded Save wait's last IME query is retained.
A setText return does not confirm the field value. Click wrapper times
do not prove actual input down/up delivery or JavaScript submit receipt.
The trace follows run43279856761, whose final image showed text selection
on Save without establishing why the first Web page was not reached.
Run43305494368 collected the trace in the installed App, then reached Work
Web login, home, Notes/draft and native Back/Home before the saved-row lookup
failure described above. Its Save click wrapper returned in 64ms; the later
Web/UI assertions establish that run's progress, not the wrapper timing alone.

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
Run43305494368 collected both bounded queries without truncation; each said
that no ANR had occurred since boot. This does not establish the earlier ANR's
cause or prove it repaired.

Run43324346638 again stopped in initial setup with the System UI dialog,
before the new native form scroll or any connection-add input. Its activity
summary reported no ANR. Its newest trace's main subject/stack concerned
Google Play startup, while the embedded CriticalEventLog identified System UI
PID1058 at Unix milliseconds1794524643642, waiting20,024ms while executing
`SystemUIService`. System UI's own stack and the cause remain unknown.
Failure-only collection now also
reads `logcat -b events -d -v threadtime,epoch,usec -s am_anr` and
`dumpsys dropbox --print system_app_anr`, retaining at most 128KiB and 512KiB
respectively under the same private-raw/size/hash/error policy. Ownership is
rechecked before each query; each additional command has one 10-second
allowance. The event record can name the ANR process and time; the DropBox tag
can contain multiple system apps, so identities/timestamps still require
matching. The allowances exclude ownership checks and CLI retirement; they
are not a total collection deadline. Missing, denied or truncated records
remain inconclusive. No logs
are cleared, permission changed or dialog dismissed. These queries have
SDK-free controls; collection on the actual guest remains unverified.
The command/filter behavior follows the Android 16
[DropBox service](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/android16-release/services/core/java/com/android/server/DropBoxManagerService.java),
[logcat](https://android.googlesource.com/platform/system/logging/+/refs/heads/android16-release/logcat/logcat.cpp)
and [ANR event fields](https://android.googlesource.com/platform/frameworks/base/+/master/services/core/java/com/android/server/am/EventLogTags.logtags).

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
