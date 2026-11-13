# Whole Tauri App iOS Web-home fixture

Source candidate; actual whole-App runtime qualification is pending. This
fixture installs the existing unsigned Simulator build and drives its real
WebHome, native AppScreen and Web pages. The tiny UIKit target only builds
the XCTest driver; its bundle identifier differs from the product build.

On a disposable Mac with the existing SDK/toolchain:

```bash
bash native-tests/ios/build-simulator.sh
python3 -B native-tests/ios/web-home-fixture/run.py --out /absolute/new/private/evidence
```

The build must produce exactly one `com.takosumi.fixture.simulator` App under
its own generated build directory. The runner fingerprints every plain bundle
file, verifies the installed copy before and after testing, and keeps the
App bundle, generated CA/key and containers outside uploaded evidence. It
uses the existing CLI retirement/scratch custody and creates, reads back,
trusts and deletes only its fresh Simulator UUID. A failed cleanup or
interrupted command prevents its final PASS. It installs no SDK or tool.
The first native attempt stopped before XCTest when installation of the
127,665,411-byte App exceeded the initial 30-second setup allowance. Installation
now has one 120-second wait, without a retry. Normal command completion and the
same full-bundle readback remain required; the original cause is unknown and
native UI qualification is still pending. The 600-second XCTest budget and
20-minute CI case cap are unchanged.

The next native attempt installed and byte-checked the whole App, then the
correct App process stopped before its automation session became ready. Its
exit cause remains unknown. Each intended launch now records the App state
before querying UI and requires foreground state; the existing form/navigation
checks remain. On failure, the runner rechecks the exact owned, booted Simulator
and queries only its Takosumi-process/private-bundle log events, keeping at most
512KiB. Host-wide logs, relaunches and extra UI waits are not used. These are
diagnostic candidates until native execution; an absent exit reason is not a
successful launch or a causal diagnosis.

The subsequent original crash report identifies a DYLD launch abort: embedded
`libswift_Concurrency.dylib` cannot resolve `@rpath/libswiftCore.dylib`. A
product-owned template based on pinned CLI2.11.4 now adds the OS Swift search
path before bundled frameworks for normal init and this SDK build alike.
This is a runtime-search repair candidate, not a native launch pass. The SDK
keeps byte-bound `otool -L/-l` evidence for the App and embedded runtime, and
the UI runner checks that proof against its actual installed-build input.

[Apple's bundle-ID initializer](https://developer.apple.com/documentation/xcuiautomation/xcuiapplication/init(bundleidentifier:))
can launch an installed app when the scheme/dependencies/build-products
contain no matching build. This driver uses a different identity and its own
DerivedData so it cannot substitute its placeholder for the Tauri App.

XCTest uses actual fields, links, buttons and native menu items. Fixture page
JavaScript renders ordinary document/storage/bridge observations; the test
injects no script, cookie, permission grant or native callback.

- Save Work through the connection form, sign in to the synthetic HTTPS
  instance, use its Web management page and Notes app icon, and exercise
  native Back, App home and connection picker.
- Require a separate editor host/origin, no App IPC, empty editor storage
  despite instance storage, and an unchanged document UUID when returning
  immediately to Work and after background/`activate()`.
- Add Personal at the same instance origin. Require its fresh authentication
  and separate editor data, then return to Work's correct identity/draft.
- Terminate and observe the App as not running, relaunch without reinstalling,
  and require both saved connections, the eligible editor route, a fresh
  document UUID and each profile's saved draft/cookie identity.
- Close both views, explicitly clear Work through the shared confirmation,
  require its root to be signed out and editor data empty, and require
  Personal's authentication and draft to remain.

Instance A uses `127.0.0.1`; editor B uses bare `localhost` with a matching
disposable certificate SAN. Only A is saved as a connection. Current Swift
Web-home navigation/resume allows B; the shell connection form itself still
rejects `localhost`. Server evidence requires A's host cookie absent at B.
This avoids treating two ports on one host as separate cookie scopes.

Run43290145178 installed and launched the actual App, added Work, signed in,
and reached its home and management page. Notes then missed the unchanged
15-second element deadline: the failure screenshot still showed Work, while
the final teardown hierarchy showed the editor on its correct host. The
request record alone did not prove when the server response finished. Each
existing request record now includes server receive/write times, status,
byte counts and a closed transport-failure class. Its response snapshot is
replaced at each stage without growing the published request or changing a
response dictionary that collection may already be reading. No headers, response bodies
or error text are recorded. Receipt is timestamped after request parsing,
not at socket acceptance or TLS handshake. A returned write does not prove WebKit commit,
paint or accessibility readiness. This diagnostic adds no request, UI read,
tap, retry or wait, and does not alter the functional or cleanup verdicts.

Run43297320017 passed the Work flow through Notes navigation, draft and warm
return, then failed after attempting Personal. Its recording shows the URL
field still empty, followed by the browser's `Fill out this field` validation
and then the completed URL. Personal was not saved and no new server request
arrived. Returning from `typeText` did not prove that the requested input had
landed. Before its single Save tap, the driver now adds a ten-second exact
value-readiness condition for each actual field. Missing input fails before
Save. The keyboard remains present; there is no replacement input,
keyboard-dismiss tap or Save retry. The separate fifteen-second Web-page
deadline is unchanged. Run43330943400 passed Work/Personal input, synthetic
Web login, management/editor navigation, profile separation, warm return and
actual process termination/cold restoration. It failed later at the Work
clear-confirmation lookup, before the deletion tap.

The saved screenshot and hierarchy show the correct Work confirmation.
WebKit exposes its full heading as one `Other` node; its `StaticText` children
split the connection name and the rest of the sentence. The old full-text
`StaticText` query therefore matched nothing. The fixture now queries that
exact full heading under `otherElements`, with the same15-second wait and
single subsequent clear tap. It does not accept a partial heading or change
product rendering. The repaired selector and actual reset/Personal survival
still require native execution. Simulator deletion succeeded in that run;
an interrupted target-log CLI required scratch retention.

Current iOS retains only one non-popup App screen. Warm document-memory
retention is checked before opening Personal; cross-profile reopening checks
persistent state and restored URL. This fixture does not claim simultaneous
live documents across profiles, whole-App/OS-crash recovery or unsaved writes
surviving eviction. Real IdP, physical devices, files/notifications, signing,
distribution and updates remain separate.

CI runs the SDK build and this case in their own `macos-26` arm64 job.
The standalone download case remains in the desktop Intel Mac job. GitHub-hosted
[jobs use separate VMs](https://docs.github.com/en/actions/concepts/runners/github-hosted-runners),
so a preceding download case's interrupted CLI or CoreSimulator service state
is not this case's setup. It builds the SDK once on the same host as its
installed-byte checks; no App bundle or Simulator state crosses jobs. Existing
compiler cache is restored read-only, with no additional cache writer.

The 60-minute job, 25-minute SDK and 20-minute case caps remain unchanged, as
do the boot/trust limits, ownership checks and strict UI/cleanup verdicts.
The separate job was introduced after run43279856761's whole-App boot failed
following a download case that passed but retained interrupted-setup scratch.
The cause of that boot failure remains unknown. Separate hosts do not guarantee
boot success. The SDK-free harness controls cannot qualify native behavior.

Run43336528310 built the Intel SDK App, but bootstatus did not confirm normal
CLI exit within the existing300-second readiness budget. Its native Finished
line reported elapsed05:03; installation and XCTest were not reached, so the
repaired clear-heading query remains unqualified. The whole-App job now uses
the build script's existing `aarch64-apple-ios-sim` path on Apple Silicon.
This selects the native arm64 SDK/UI qualification path; it does not establish the earlier
boot failure's cause. The same fixture, owned fresh Simulator, installed-byte
checks, UI actions, deadlines and cleanup verdict remain. No Intel fallback
or same-head rerun is added. The standard arm64 runner has3 CPU cores and7GB
RAM according to [GitHub's runner table](https://docs.github.com/en/actions/how-tos/write-workflows/choose-where-workflows-run/choose-the-runner-for-a-job);
the build/case caps are not increased, and actual arm64 build/runtime results
are still pending.
