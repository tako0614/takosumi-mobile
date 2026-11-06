# Standalone browser-foundation candidate

Experimental Linux CEF 154 process using the App's production `OpenRequest`
validator and UUID profile IDs. It does not replace the Tauri runtime or change
installed App profiles. See [upstream attribution and scope](UPSTREAM.md).

Build with an owned SDK/target directory and the committed candidate lockfile:

```bash
CEF_PATH=/absolute/owned/sdk CARGO_TARGET_DIR=/absolute/owned/target cargo build --locked --manifest-path native-tests/cef-candidate/Cargo.toml --bin takosumi-cef-candidate
python3 -B native-tests/cef-candidate/run.py --binary /absolute/owned/target/debug/takosumi-cef-candidate --data /absolute/new/case --app photopea
```

The first build downloads a separate CEF SDK. Check available storage first; do
not use the product's Cargo target, install shared OS packages or alter the App
lockfile. The executable needs its adjacent SDK libraries/resources. The runner
sets its library search path, creates a private HOME/XDG/Xvfb/D-Bus session and
uses the existing pidfd browser guard. Node 24+, Python, Xvfb, D-Bus, `timeout`
and ImageMagick `convert` must already be available. Only anonymous Photopea and
Excalidraw roots and generated files are allowed; no real accounts or cloud save.
Use a short `--data` path: the pinned Linux Chromium's temporary singleton
socket must fit `sockaddr_un`. The runner rejects an oversized path before
allocating a case or starting the browser; it does not relocate profile data.

Default `--files cdp` uses a headless browser except in the permission modes
below. It supplies the site's actual
existing file input through CDP and sets `allowAndName` for an owned download
directory. Success requires the actual selected page's download GUID/frame,
native completion, decoded exported data, no remote native IPC globals and
normal CEF shutdown with no remaining child. Photopea's PNG must contain the
fixture's dimensions and red/blue pixels. This is a component check, with the
OS chooser and default Save UI unqualified.

`--mode inspect` records observations and never qualifies an operation.
`--app excalidraw --files native-picker` is a separate visible-browser probe of
the actual File System Access Open and Save As choosers; it additionally needs existing `xdotool`
and ImageMagick `import`. Its experimental English GTK dialog driver must fail
when the expected unique dialog/focus/closure is missing. It does not disable
Web APIs or substitute another input route after a failed operation. Retain the
failed case rather than inferring engine incapability from an automation error.
The driver waits for the rendered confirm label in the inspected GTK theme
before clicking once, and replaces the entire filename including its extension.
This pixel heuristic is specific to that theme/geometry. A red canvas is only
import readiness; success requires the exact new owned Save As output, the
case-specific scene nonce and the actual keyboard edit. A toast or a file saved
under an unexpected name cannot pass. This path does not require a download
event because File System Access writes through the selected native file handle.

`source.json` and `source-snapshot/` retain the runner, driver, export oracle and
cleanup validators; `execution.json` also checks their hashes, the executable
and `libcef.so` after the session. `reference.json` contains the component result.
Every qualifying mode requires `execution.passed`. `operate` also requires
`reference.operationPassed`. Both profile modes require `reference.profilePassed`; `profile-popup` additionally
requires `reference.popupPassed`. Inspect never qualifies a capability. All
success flags also require root exit 0, no remaining child and no forced cleanup.
`site-permissions` requires `reference.profilePassed` and
`reference.permissionPassed` as well as `execution.passed`.
`permission-inspect` never qualifies a permission or notification capability.
`site-notifications` additionally requires `reference.notificationUiPassed`;
`reference.pushPassed` is always false in these local probes.

## Native profiles and popup primitive

```bash
python3 -B native-tests/cef-candidate/run.py --binary /absolute/owned/target/debug/takosumi-cef-candidate --data /absolute/new/profile-case --app photopea --mode profiles
python3 -B native-tests/cef-candidate/run.py --binary /absolute/owned/target/debug/takosumi-cef-candidate --data /absolute/new/popup-case --app photopea --mode profile-popup
```

These modes create two explicit UUID request contexts in one native process;
they do not use CDP context/target creation as a replacement. Native BrowserHost
cache-path readback is joined to the actual page through native and CDP main
frame IDs. Native DevTools registrations live until their browser closes. The
binding is captured after the first main-frame load: an early inspection target
may still have type `other`. URL, target order or configured paths alone do not
qualify a binding. Unknown multi-profile Chrome-UI windows receive a close-only
client, rather than an unmanaged window or an inferred profile.

The test writes a case-specific localStorage key and synthetic Secure/HttpOnly
cookies into the fresh contexts. Cookie setup uses CDP, not a server login.
Wrong state fails on its single read; only lifecycle/readiness metadata is
polled. Actual native close callbacks and target disappearance precede retained
state checks in the surviving profile.

`profile-popup` drives standard `window.open` / `window.close` through CDP
`Runtime.evaluate(userGesture: true)`. It checks native profile/cache/frame and
browser-context inheritance, the actual target's opener, JS opener presence and
absence of native messaging globals. The child reads Work state, writes new
synthetic state, closes, and its still-live parent reads the new state while
Personal remains separate. It does not qualify a real click, OS focus, popup
blocking policy, cross-origin/provider authentication or login completion.

## Default site permission UI

```bash
python3 -B native-tests/cef-candidate/run.py --binary /absolute/owned/target/debug/takosumi-cef-candidate --data /absolute/new/permission-case --app excalidraw --mode site-permissions
TMPDIR=/absolute/owned/tmp node --test native-tests/cef-candidate/permissions.test.mjs
```

`permission-inspect` only requests notification permission and retains the
native screenshot and state. `site-permissions` exercises Chrome's existing
Allow/Block UI in a visible owned Xvfb session. Both modes additionally need
existing `xdotool` and ImageMagick `import`. A generated document title binds
the actual page to a unique visible window whose PID must equal the owned
native process. Each window is placed in the inspected geometry, with both
CDP and X11 geometry readback. The click coordinates are specific to this
English Linux build and the separately inspected single-line Excalidraw and
wrapped Photopea prompt layouts; this is not a portable prompt driver.

The standard permission request uses CDP's synthetic userGesture, but the
decision uses one native UI click. No permission handler, site API replacement
or CDP grant is installed. Before every choice the request must still be
pending without error at the original secure origin. All permission samples
must retain that bound origin. Work's Allow must resolve to granted while
Personal remains prompt; Personal's Block must resolve to denied while Work
remains granted. Seven replay controls reject wrong/insecure sites,
redirected requests and pre-resolved or redirected pre-click snapshots before
any native click, and check native PID binding and process-local command routing;
they are oracle tests, not native capability evidence.

On 2026-11-05 Excalidraw passed the choices, state separation and normal CEF
shutdown. Photopea's initial probe showed its real prompt, and an earlier
partial run granted Work while Personal remained prompt, but its complete
final flow is unqualified. Window-cascade/layout and cold-start failures are
retained. Permission revocation, restart persistence and other permission
types are still unfinished.

The permitted notification constructor emitted `show`, while the denied
profile emitted `error`. The owned session nevertheless reported a missing
`org.freedesktop.Notifications` service. A session also printed an unattributed
`free(): invalid pointer` outside the CEF log; normal CEF exit does not qualify
the entire desktop service stack. `notificationPassed` remains false: a page
event is insufficient for OS display, actual Push delivery, click or resume.

## Owned native notification backend

```bash
python3 -B native-tests/cef-candidate/run.py --binary /absolute/owned/target/debug/takosumi-cef-candidate --data /absolute/new/notification-case --app photopea --mode site-notifications --notification-daemon /absolute/owned/dunst
TMPDIR=/absolute/owned/tmp node --test native-tests/cef-candidate/notifications.test.mjs
```

This explicit mode needs an already available Dunst 1.12.2 executable, `gdbus`,
`dbus-monitor`, X11/XRes 1.2 libraries, `xdotool` and ImageMagick `import`. It
starts that real daemon only in the runner's fresh private D-Bus/Xvfb session;
it neither registers an installed App service nor changes shared OS packages.
The executable hash is recorded and checked after the run. The backend's real
D-Bus owner PID and XRes server resource-owner PID must match the owned daemon.
It preserves normal tag/replacement handling and uses one visible plus one
waiting notification. The page API and desktop signals are never substituted.

Both native UUID contexts receive an actual Allow choice in Chrome's existing
notification prompt. Generated standard page Notifications use the same tag
on the same origin, with different Work/Personal titles. Success would require
coexistence, live page objects, native screenshots/input, trusted clicks only
in the original profile, actual desktop action signals and native closure.
Before each click the other owned browser window must have focus; only the
page's standard onclick `window.focus()` may restore its own window. The test
does not repair focus through CDP or X11 after the click. Network Push,
Service Worker background delivery and closed-App resume are separate.

The 2026-11-05 two-profile checks **failed**. Excalidraw's visible Work alert
produced a native action for its desktop notification ID, but Personal received
the callback. After strengthening state capture and the focus precondition,
Photopea failed before clicking: Personal's same-tag creation had already
closed Work's JS notification object while its OS alert remained displayed.
Both CEF guards and owned daemons shut down normally; these are functional
failures, with `notificationUiPassed` and `execution.passed` false. Five replay
controls reject wrong-profile callbacks, closed page objects, foreign X11
owners, an already-focused target and an occluded input point; they are not
native evidence.

The matching Chromium source uses origin plus the non-persistent notification
token as the ID; a nonempty tag becomes that token. Its process-wide listener
registry replaces entries by this ID. This is consistent with the observed
collision across isolated contexts. A future adapter must preserve profile
identity before this listener boundary, or evaluate separate browser processes
per profile. Merely routing the OS action by profile cannot recreate an already
replaced listener. The default single-process mode applies no such fix; the
same-tag failure remains a required regression. The explicitly selected
process comparison below does not replace the installed App runtime.

## Process-per-profile adapter comparison

```bash
python3 -B native-tests/cef-candidate/run.py --binary /absolute/owned/target/debug/takosumi-cef-candidate --data /absolute/new/process-case --app photopea --mode site-notifications --process-layout per-profile --notification-daemon /absolute/owned/dunst
```

This notification-only mode reuses the same executable, native request
validator and UUID profile IDs in two separately guarded browser processes.
Each has its own HOME/XDG/cache root, PID-owned CDP socket and native frame
binding. Window IDs and browser commands remain scoped to the originating
process. It does not add a native IPC bridge to either remote page, change
website tags, migrate installed profiles or change the Tauri UI/mobile-kit.

Excalidraw and Photopea both passed this local component on 2026-11-05: both
same-tag notifications coexist, each native click reaches only its original
profile as a trusted event, and the page's standard onclick `window.focus()`
restores its exact native window from a verified other-window focus. The
actual pointer target must equal the XRes-attested daemon XID before the single
click; the fixture never raises the daemon or repairs focus afterward. In
this WM-less session the restored browser can partially cover the next alert;
the right title area remains visible and hit-tested. These screenshots do not
qualify notification presentation under a real desktop window manager.

After both clicks, exact native frame and once-only document nonce, synthetic
localStorage and Secure/HttpOnly test cookies must remain. Work then closes
through its native target and full normal guard before Personal's retained
document/state is read once. Both guards, daemon stop and outer unchanged-hash
checks must pass along with all three reference flags. Two initial Excalidraw
cases remain failed: an occluded input point, then `mousemove --sync` waiting
for motion at an already occupied coordinate. No timeout was increased; the
driver now queries the actual pointer target instead.

The current-source single-process negative comparison still fails before
clicking with Work's page object closed. The process comparison demonstrates
a viable isolation boundary for this component, without establishing unique
causation for every earlier failure, a production adapter, runtime selection,
real site-generated Push, Service Worker delivery, closed-App resume, durable
profiles, performance or other OS support. The existing Dunst prerequisite
remains test-only. No CEF/Chromium source or SDK patch is applied.

On 2026-11-05 the basic same-origin profile test and the synthetic popup flow
passed on native CEF, including normal closure. Earlier popup runs passed the
functional assertions but failed shutdown and remain failures. Milestones
located a recorder EOF wait after SDK shutdown/client release; the recorder now
ends on the explicit runtime-complete boundary and is still joined. This does
not establish that all retained SDK references or cold-start issues are solved.

## Actual Tauri shell and browser-process experiment

An explicit Linux fixture now connects the existing bundled WebHome and its
shell-only `app-screen.open` command to a separate CEF process per UUID profile.
Normal builds keep their existing runtime. No shared-kit or remote page IPC is
added, and installed profiles are not migrated. This is an integration probe,
not an engine decision or a distributable browser service.

```bash
CEF_PATH=/absolute/owned/sdk CARGO_TARGET_DIR=/absolute/owned/target cargo build --locked --manifest-path native-tests/cef-candidate/Cargo.toml --bin takosumi-cef-browser-process
cargo build --locked --manifest-path src-tauri/Cargo.toml --features native-browser-process-fixture
python3 -B native-tests/desktop/browser_process.py --binary src-tauri/target/debug/takosumi-mobile --browser /absolute/owned/target/debug/takosumi-cef-browser-process --data /root/dev/tmp/new-short-app-case --app excalidraw
```

Run `--app photopea` in another fresh case for the second anonymous origin.

`takosumi-browser-guard` (`src/guard_main.rs`) is the product supervisor that
replaces the fixture's Python guard for the Linux-first per-profile release. It
keeps `guard.py`'s arguments and receipt: an exclusive 0600 receipt, its own
process group, a pidfd probe, the child subreaper, SIGTERM on parent death,
pidfds retained only after a stable-ancestry re-check, the bounded drain
(0.5 s natural, 0.5 s SIGTERM, 2 s SIGKILL) and ECHILD as the completion proof.
It does not cap the browser's lifetime, and while the browser runs it blocks on
a signalfd with a 250 ms observation tick instead of polling every 20 ms. The
adapter runs a guard path ending in `.py` with `python3 -B` and executes any
other guard directly; the fixture runner still passes `guard.py`, so the Rust
guard is not yet qualified in the actual App run. Its self-tests use stand-in
children only and never start CEF:

```bash
CEF_PATH=/absolute/owned/sdk CARGO_TARGET_DIR=/absolute/owned/target cargo build --locked --manifest-path native-tests/cef-candidate/Cargo.toml --bin takosumi-browser-guard
CEF_PATH=/absolute/owned/sdk CARGO_TARGET_DIR=/absolute/owned/target cargo test --locked --manifest-path native-tests/cef-candidate/Cargo.toml --bin takosumi-browser-guard
```

The runner needs the existing Python pidfd/subreaper guard, Node, Xvfb, D-Bus,
XRes, xdotool and ImageMagick; it installs nothing. It supplies fresh HOME/XDG
directories and explicitly enables PID-attested loopback inspection ports.
The browser control pipe accepts only bounded validated open/close/shutdown
requests, never JavaScript or arbitrary paths. Pipe writes and reply waits
share a bounded deadline. The child retains a native CEF observer and enables
Page observation only when a single-window close is requested. Native setup
must succeed before closing; refusal/failure retains the browser and blocks
later unobserved closes. It observes the default beforeunload UI's result; it neither
replaces that UI nor exposes DevTools commands through the parent pipe.

The driver inspects the shell-created processes, native profile/cache/window
and XRes owner, actual document, absent page IPC, separate synthetic storage,
and exact document retention after a real shell resume. After a cancelled close
has enabled the native observer, it also dismisses a 9,001-character native
alert and checks that the document survives. Beforeunload
setup uses synthetic CDP user activation; cancellation uses native Escape.
The App must then resume the original Work document through its existing UI
and exit normally, with both browser guards and the outer session guard clean.
`execution.passed` requires the App/observer flags, normal receipts, and unchanged
source/artifacts; merely creating a window or writing a completion marker fails.

Failure capture records the pending command, bounded frame/dialog/transport
events, an owned-display screenshot and two kernel samples of the exact guard
generation. It verifies start identity, native executable and stable ancestry
before reading process/thread state; it neither signals processes nor reads
target argv/environment or browser storage. A denied snapshot stays unknown and cannot qualify
the browser. `python3 -B native-tests/desktop/test_browser_snapshot.py` runs
bounded-hashing, source-scope, cleanup-failure and live/stale/foreign-generation
controls without CEF/network. The outer runner retains explicitly selected Linux
fixture sources, tracked frontend/app-screen/capability inputs and executable hardlinks per case, hashes
large artifacts with 1 MiB reads, and writes the original failure even when
cleanup also fails. Transport closure rejects pending commands directly.

A later diagnostic startup failure retained a blank document and native
thread waits in `jbd2_log_wait_commit`, followed by incomplete child cleanup.
That is evidence of an I/O wait in that case, not a unique cause of all earlier
Photopea/startup failures or an engine-wide incompatibility. No timeout or
filesystem/global concurrency setting is changed to turn it into a pass.

**Coordinated quit is unsupported:** the parent refuses a quit while multiple
profiles are live, and a child refuses shutdown with multiple native screens.
The refusal happens before any close. Native browser windows must be closed
individually first; the fixture verifies both original documents survive the
multi-profile refusal, closes Personal through Chrome's native shortcut, then
tests Work's cancellable ordinary App exit. A production quit UX, cross-window
beforeunload coordination, profile reset/migration, restart persistence, popup
authentication, files/permissions/notifications in this new App path, sandbox,
packaging/update and other OSes remain unfinished. The Python guard and the
root-only unsandboxed setup are fixture prerequisites, not release design.

The Excalidraw ex5 actual-App flow passed, including native cancellation,
an active-observer long alert and normal guards, on browser binary
`2b94920c8a91cf24f908e5db54832219de33c9af84aa1c5fd00579dc840107e3`.
The later failed-observation state repair compiled as
`47f99c9ca8ee99849031e6f0ce5fb735429fa1d30ddd4405ebd4e1fc4d3437f2`,
but its ex6 actual-App run remained at `about:blank` and failed before close
was exercised. The earlier pass does not qualify the latest source or binary.
Photopea remains unqualified in this path: its landing page
and two visible equivalent start buttons were observed, but after the
hit-tested start operation `Runtime.evaluate` stopped responding within the
existing five-second bound. Deferring the internal Page agent until close did
not remove the observed failure. No unique cause, CEF-wide inability or
Photopea compatibility is inferred. These failed App cases do not erase the
earlier standalone component results or establish App-wide file/notification
support. Retain each exact-source receipt when comparing later candidates.

## Observations and limits

On 2026-11-05 the native candidate loaded Photopea, imported its generated SVG,
saved and decoded a 138-byte 128×96 PNG and shut down normally. The original
private driver and the public runner each passed separately. Excalidraw also
passed actual native Open, import, keyboard edit and native Save As to the exact
new output, with the nonce and edited geometry verified in a 961-byte scene and
normal shutdown. Earlier headless chooser, disabled-confirm, title-readiness
and doubled-extension failures remain failures. Some cold starts stopped at
about:blank with incomplete guard cleanup; kernel I/O waits were observed and
the retained PIDs were later absent. Their unique cause remains unresolved.
These successful file operations do not establish daily-use reliability.

The native browser reports non-global request contexts whose cache paths equal
the requested UUID directories. Same-origin synthetic storage/cookie separation
and the popup primitive do not establish permission isolation, restart
persistence or real authentication. Production Tauri integration, real login/popup
return, other site permissions/revocation, Web Push delivery, sandboxing, other OSes,
updates and distribution remain unfinished. The runner
uses `no_sandbox` in its disposable root test environment; it is not a release
configuration or an engine-selection decision.
