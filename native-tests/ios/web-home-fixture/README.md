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

Current iOS retains only one non-popup App screen. Warm document-memory
retention is checked before opening Personal; cross-profile reopening checks
persistent state and restored URL. This fixture does not claim simultaneous
live documents across profiles, whole-App/OS-crash recovery or unsaved writes
surviving eviction. Real IdP, physical devices, files/notifications, signing,
distribution and updates remain separate.

CI runs the case independently of the download fixture after the SDK step
succeeds. It retains the Mac job's 60-minute cap and adds a 20-minute case;
the preceding observed Mac job took 25m18s. That measurement is capacity
planning, not a guarantee or permission to widen deadlines. The harness
controls are SDK-free and cannot qualify native behavior.
