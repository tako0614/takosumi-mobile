# Standalone browser-foundation candidate

Experimental Linux CEF 154 process using the App's production `OpenRequest`
validator and UUID profile IDs. It does not replace the Tauri runtime or change
installed App profiles. See [upstream attribution and scope](UPSTREAM.md).
The per-profile browser process also has a first Windows candidate; see
[Windows browser process](#windows-browser-process-first-candidate).

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
The same driver runs on Windows against a local fixture site; see
[Windows actual App (W3)](#windows-actual-app-w3), and against real sites in
a manual run: [Windows real sites (W4)](#windows-real-sites-w4-manual-evidence-run).
Normal builds keep their existing runtime. No shared-kit or remote page IPC is
added, and installed profiles are not migrated. This is an integration probe,
not an engine decision or a distributable browser service.

```bash
CEF_PATH=/absolute/owned/sdk CARGO_TARGET_DIR=/absolute/owned/target cargo build --locked --manifest-path native-tests/cef-candidate/Cargo.toml --features fixture-inspection --bin takosumi-cef-browser-process
cargo build --locked --manifest-path src-tauri/Cargo.toml --features native-browser-process-fixture
python3 -B native-tests/desktop/browser_process.py --binary src-tauri/target/debug/takosumi-mobile --browser /absolute/owned/target/debug/takosumi-cef-browser-process --data /root/dev/tmp/new-short-app-case --app excalidraw
```

Run `--app photopea` in another fresh case for the second anonymous origin.
The fixture browser needs the cef-candidate feature `fixture-inspection` for
its observation seam (see [Linux packaging](#linux-packaging-desktop-cef)).

`takosumi-browser-guard` (`src/guard_main.rs` on Linux, chosen by
`src/guard_entry.rs`; the Windows guard is described under
[Windows guard and adapter](#windows-guard-and-adapter)) is the product supervisor that
replaces the fixture's Python guard for the Linux-first per-profile release. It
keeps `guard.py`'s arguments and receipt: an exclusive 0600 receipt, its own
process group, a pidfd probe, the child subreaper, SIGTERM on parent death,
pidfds retained only after a stable-ancestry re-check, the bounded drain
(0.5 s natural, 0.5 s SIGTERM, 2 s SIGKILL) and ECHILD as the completion proof.
It does not cap the browser's lifetime, and while the browser runs it blocks on
a signalfd with a 250 ms observation tick instead of polling every 20 ms. For
a long-lived browser it holds pidfds only for processes that still exist,
checked through the pidfd itself (`observed` counts every distinct owned
process), records observation failures, a missing browser pidfd and reap or
wait failures that recover within eight ticks in `nonFatalErrors` instead of
stopping the browser, signals the unreaped browser by pid if it has no pidfd,
withholds
descriptors leaked into it from the browser, resets inherited SIGTERM, SIGINT
and SIGCHLD dispositions, starts the browser with no blocked signals and its
own original descriptor limit, and starts nothing when a stop is already
pending. The adapter arms the guard's parent-death signal between fork and
exec, runs a guard path ending in `.py` with `python3 -B` and executes any
other guard directly; `browser_process.py --guard` selects the Rust guard, which
is not yet qualified in the actual App run. Its self-tests use stand-in
children only and never start CEF:

```bash
CEF_PATH=/absolute/owned/sdk CARGO_TARGET_DIR=/absolute/owned/target cargo build --locked --manifest-path native-tests/cef-candidate/Cargo.toml --bin takosumi-browser-guard
CEF_PATH=/absolute/owned/sdk CARGO_TARGET_DIR=/absolute/owned/target cargo test --locked --manifest-path native-tests/cef-candidate/Cargo.toml --bin takosumi-browser-guard
```

`browser_process.py --recover` is a separate same-App crash-recovery case; do
not combine it with restart/reset/files/notification scenarios. Pass the
product Rust guard explicitly with `--guard /absolute/owned/target/debug/takosumi-browser-guard`
(or use `--packaged` with that guard). The Python fixture guard lacks the complete
incident schema required for recovery. With either `--app excalidraw` or
`--app photopea`, the case saves distinct cookies/storage in Work and Personal,
normally closes and reopens Work to prove its data was saved, then terminates
only that exact native Work generation through a pidfd. It requires the guard's
abnormal receipt with kernel-confirmed exhaustion, the home's explicit recovery
action, unchanged UUID/cache-directory inodes and saved values, an immutable
incident copy, and Personal's original process/document/data. Final live browser
guards must still exit normally; the induced crash is never relabelled normal.
The existing finite watchdogs, document-readiness predicate and source-hash
checks remain unchanged. This Linux fixture is not Windows/macOS, power-loss,
real login or unsaved-write qualification.

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
authentication, files/permissions/notifications in this new App path,
sandbox qualification, update and other OSes remain unfinished. The sandbox
decision and the local deb candidate are described under
[Linux packaging](#linux-packaging-desktop-cef). The Python guard and the
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

## Linux packaging (`desktop-cef`)

The App feature `desktop-cef` enables this per-profile browser path in a normal
App build, without the fixture driver; `native-browser-process-fixture` now
implies it. This section is about Linux; on Windows the feature builds the
unsandboxed candidate, packaged since W6 (see
[Windows packaging (W6)](#windows-packaging-w6)). It is never enabled
by default, and nothing here publishes or signs a package. The first local packaging default (chosen by the
coordinator, not an approval to publish or distribute) is a deb plus an AppImage.

**Locations** (`src-tauri/plugins/app-screen/src/desktop/process_setup.rs`).
`TAKOSUMI_BROWSER_PROCESS_PROGRAM`, `TAKOSUMI_BROWSER_PROCESS_GUARD` and
`TAKOSUMI_BROWSER_PROCESS_RUNTIME` stay explicit overrides, and the fixture
sets all three. They are honoured only with the plugin feature
`fixture-overrides` (enabled by `native-browser-process-fixture`); a
`desktop-cef` product build ignores them, like the sandbox opt-out and the
loopback inspection seam below. Unit tests cover both feature sets without
changing the process environment. Without overrides, the browser and guard are
`<resource_dir>/cef/takosumi-cef-browser-process` and
`<resource_dir>/cef/takosumi-browser-guard`; both must canonicalize to
existing files. On Linux, Tauri's resource directory is `<exe dir>/../lib/<productName>`
when that exists, otherwise `$APPDIR/usr/lib/<productName>`, otherwise
`/usr/lib/<productName>`; a binary in a Cargo `target/<profile>/` directory
uses that directory. That gives `/usr/lib/Takosumi/cef/` for the installed
deb, `<tree>/usr/lib/Takosumi/cef/` for an extracted deb, and
`$APPDIR/usr/lib/Takosumi/cef/` (inside the AppImage mount) for the AppImage.
For a `cargo` or `tauri dev` build, tauri-build copies the overlay's resources
into `target/<profile>/cef/`.

The runtime root holds each browser's sockets and `TMPDIR`. The App uses
`$XDG_RUNTIME_DIR/takosumi-cef` only when `XDG_RUNTIME_DIR` is absolute and,
per the XDG Base Directory rules, a directory owned by the effective user with
mode 0700, and only when `takosumi-cef` there is missing (it is created 0700)
or already a plain, private directory of that user. Otherwise it uses
`<app cache>/cef` (normally `~/.cache/com.takosumi/cef`). That covers a
root-owned `takosumi-cef` left by `sudo -E`, another user's runtime directory
after `su`, a shared `XDG_RUNTIME_DIR=/tmp`, and a runtime path too long for
the socket. A skipped directory is never chmod-ed or chown-ed. Every root,
including the fixture's override (which is never created or replaced), must
be a plain directory (not a link) owned by the effective user, with no group
or other permission bits. It must also be short enough that
`p-<pid>-999999/tmp/org.chromium.Chromium.XXXXXX/SingletonSocket` below it
fits in 108 bytes.

If no browser, guard or runtime root qualifies, or the App runs setuid or
setgid (`getuid() != geteuid()`, or the same for the group), the plugin still
starts. The backend is registered as unavailable with its reason, printed to
stderr. The home keeps running, and `open` and `clear_profile` return that
reason; nothing falls back to the Wry adapter. `capabilities` then reports
`clearProfile: false`, so the home does not offer a deletion that would be
refused.

Each browser's `p-<App pid>-<n>` directory holds a `lock` file. The App keeps
an exclusive `flock` on it for that browser's whole lifetime and releases it
only after the directory is removed. Liveness is therefore decided by the lock,
not by a pid, so another host sharing an NFS cache fallback, or another PID
namespace sharing `/run/user/<uid>`, keeps its directories. A directory is
removed once its guard has ended: callers drop a browser right after verifying
its normal receipt, and any other ended browser is dropped at the App's end.

At start, the App also sweeps its own verified root. It removes a plain
directory of its user named `p-<pid>-<n>` only when a non-blocking `flock` on
its lock file succeeds, and removes it while holding that lock. It leaves a
held lock, a missing lock file (a directory being created, or one from an
earlier build), a lock file that is a link, links and other entries. A new
browser's name is used only when it is free or stale by the same test;
otherwise the App takes the next name. No link is followed. A fixture build
keeps ended directories, because the runner reads each guard's receipt there
after the App exits; the sweep still applies.

A browser that agreed to stop has 30 s (`STOP_FINISH`) to end normally. That
covers an answered `stopped`, a control pipe or reply channel that closed after
its last window, and a late `stopped` found while recovering. pk1 showed a
browser still flushing its profile on a stalled disk past the earlier 2–5 s
bounds. A shutdown request that could not be written within its 20 s write
bound (the browser never read it, so it never agreed to stop) fails at once,
leaving the browser in doubt; only a closed pipe waits `STOP_FINISH`. The
runner waits `STOP_FINISH` plus 15 s for the App's exit, and correspondingly
longer for the whole session, so it never kills an App that is still within
that bound.

**Sandbox.** The adapter adds `--no-sandbox` only when the App runs as root
(Chromium refuses its sandbox to root; the fixture runs as root unless the
runner's `--user` is given) or, in a fixture build only, when
`TAKOSUMI_BROWSER_PROCESS_NO_SANDBOX=1` is set. The browser sets CEF's
`no_sandbox` from the same switch, or when its own euid is 0. Otherwise
Chromium uses its user-namespace sandbox. `chrome-sandbox` is not packaged:
it works only as a setuid-root helper, which neither bundle installs. A host
that blocks unprivileged user namespaces cannot start the sandboxed browser,
and the App does not fall back to an unsandboxed one.

Ubuntu 24.04 and later restrict user namespaces through AppArmor unless a
profile allows the executable. The deb ships an allow-everything profile that
only names the browser and grants `userns`, like the profiles Ubuntu ships for
Chrome and Chromium (`src-tauri/linux/takosumi-cef-browser.apparmor`,
installed as `/usr/share/takosumi/apparmor/takosumi-cef-browser`). The
post-install script copies it into `/etc/apparmor.d` whenever
`/etc/apparmor.d/abi/4.0` exists and `apparmor_parser -Q -T -K` accepts the
shipped copy. That holds whatever the running kernel is, so an install in a
chroot without securityfs, or before rebooting into a newer kernel, finds the
profile at boot. It loads the profile now (`-r -W`) only when the running
kernel mediates user namespaces
(`/sys/kernel/security/apparmor/features/namespaces/userns_create`). AppArmor
3.x (Debian 12, Ubuntu 22.04) cannot parse `abi <abi/4.0>` or `userns`, and an
unparsable file in `/etc/apparmor.d` can make `apparmor.service` fail at boot,
so it is skipped there. Tauri's deb bundler cannot declare a dpkg trigger, so
after an upgrade from such a release the package must be reinstalled (for
example `apt install --reinstall takosumi`) to install the profile. The
post-remove script unloads and removes the installed copy on remove and purge.
On purge it also removes the compiled cache entries named
`takosumi-cef-browser`: in `apparmor_parser --print-cache-dir`, under each
`/var/cache/apparmor/*/` and in the pre-3.0 `/etc/apparmor.d/cache`, never
following links or touching other files. Both scripts pass `sh -n`, `dash -n`
and shellcheck. The profile passes `apparmor_parser -Q -T -K` without being
loaded; the scripts were not executed.

**Known limits.** An AppImage cannot install a profile. On a host that restricts
unprivileged user namespaces, its browser cannot start, and the App reports
only the browser's generic start failure, not the cause. A person or
administrator must allow unprivileged user namespaces for it. During a deb
upgrade, dpkg replaces `/usr/lib/Takosumi/cef/` under a running App. Browsers
started before the upgrade keep the old files, while later starts use the new
browser and guard with the old App, so the App should be restarted after an
upgrade. Neither case is handled in code yet.

**Build and stage.** On Linux `build.rs` gives both browser executables a
`$ORIGIN` run path, so they find the adjacent `libcef.so` without `LD_LIBRARY_PATH`;
the adapter still sets it. The browser's DevTools inspection seam (a loopback
port plus load markers for `TAKOSUMI_BROWSER_PROCESS_INSPECT=1`) is compiled
only with the cef-candidate feature `fixture-inspection`. Build fixture
browsers with it, including the stage behind `browser_process.py --packaged`.
Build a browser staged for a package without it, so the shipped executable
cannot open a DevTools port even if its environment asks. The staging script
refuses a browser containing the `TAKOSUMI_BROWSER_PROCESS_INSPECT` bytes
unless `--fixture` is given. A fixture stage defaults to
`src-tauri/target/linux-cef-fixture` (mapped by
`src-tauri/tauri.linux-cef-fixture.conf.json`), is never written to the package
stage path (even through a link), and is recorded as such in its manifest. It
strips ELF files into the stage and symlinks the unchanged data files
(`--copy` copies them instead). It stages the two executables, `libcef.so`,
`icudtl.dat`, the V8 snapshot, the `.pak` files, all `locales/*.pak`,
SwiftShader/Vulkan (and ANGLE when present) and `CREDITS.html`. It does not
stage `chrome-sandbox` or SDK build inputs. A manifest with each file's size
and SHA-256 is written beside the stage as a new file; an existing one is
refused. `src-tauri/tauri.linux-cef.conf.json` maps
`src-tauri/target/linux-cef/cef/` to the bundle's `cef/` and adds the deb
dependencies of `libcef.so`, the AppArmor profile and the two scripts. The
dependency names are the pre-t64 names, which the t64 packages provide.

```bash
# Product stage (no inspection seam):
CEF_PATH=/absolute/owned/sdk CARGO_TARGET_DIR=/absolute/owned/target cargo build --locked --manifest-path native-tests/cef-candidate/Cargo.toml --bin takosumi-cef-browser-process --bin takosumi-browser-guard
bun scripts/stage-linux-cef.mjs --cef /absolute/owned/sdk/154.0.34/cef_linux_x86_64 --bin /absolute/owned/target/debug
bunx tauri build --debug --bundles deb,appimage --features desktop-cef --config src-tauri/tauri.linux-cef.conf.json
# Fixture browser (browser_process.py --browser, or a stage for --packaged):
CEF_PATH=/absolute/owned/sdk CARGO_TARGET_DIR=/absolute/owned/target cargo build --locked --manifest-path native-tests/cef-candidate/Cargo.toml --features fixture-inspection --bin takosumi-cef-browser-process --bin takosumi-browser-guard
bun scripts/stage-linux-cef.mjs --fixture --cef /absolute/owned/sdk/154.0.34/cef_linux_x86_64 --bin /absolute/owned/target/debug
# tauri-build copies the fixture stage into target/debug/cef/ for --packaged:
TAURI_CONFIG="$(cat src-tauri/tauri.linux-cef-fixture.conf.json)" cargo build --locked --manifest-path src-tauri/Cargo.toml --features native-browser-process-fixture
```

The stage must not exist yet; the script never replaces or removes files. To
keep it outside the checkout, make `src-tauri/target/linux-cef` a symlink to
an owned directory before staging. Disk use per debug candidate is large. The
App crate alone writes about 2.2 GB per feature set: a 1.2 GB staticlib, a
357 MB cdylib, a 197 MB rlib and a 425 MB debug executable. tauri-build then
copies 360 MB of resources into `target/debug/cef/`, and the deb directory
holds an unpacked tree plus `data.tar.gz`.

**Runner with `--user`.** `browser_process.py --user <account>` runs the
session (Xvfb, D-Bus, App, observer) as that unprivileged account, so the
browsers run with the Chromium sandbox. The case root stays root's (0711),
with root's receipts, logs, hard-linked executables, source snapshot and the
recording opener. Everything the session writes lives in one session
directory, `<case>/<case name>/`: App, worker and observer artifacts, the
opener log, profile data and runtime directories. Naming it like the case
keeps the observer's synthetic nonce the case name, so the oracles are
unchanged. Only that directory is handed over. Root creates its own files
with `O_EXCL|O_NOFOLLOW` and reads session files as untrusted data: no link at
any component, regular files only, with a size bound.

**2026-11-06 local candidates.** The first stage was 232 files, 358 MiB,
of which 268 MiB were written; the stripped `libcef.so` is 272 MB, down from
1.46 GB. That deb, `Takosumi_0.1.0_amd64.deb`, was 261 MB (Installed-Size
781,983 KiB). It contained `/usr/bin/takosumi-mobile` (the 425 MB debug
executable) and `/usr/lib/Takosumi/cef/` with the two executables (0755),
`libcef.so`, SwiftShader/Vulkan, the data files and 220 locales. The desktop
entry and icons were in `/usr/share`. All 237 files in the extracted tree
matched the deb's md5sums. The extracted App started in a private Xvfb with
private HOME/XDG directories, created `$XDG_RUNTIME_DIR/takosumi-cef` (0700)
and its profile root, so the packaged files resolved. That build predated the
fallback, the unavailable state and the fixture-only gating: a 46-byte
runtime root and a missing override executable then stopped it at start. A
product build now ignores the override, and an unusable root falls back or
leaves the App running with the reason.

The packaged guard and browser also ran outside the App as `nobody` in a
private Xvfb, booting to an unresolvable `.invalid` origin. Without
`--no-sandbox`, zygote and renderer processes ran in nested user and PID
namespaces. Eight of ten sandboxed runs and all three `--no-sandbox` runs shut
down normally (`stopped`, guard exit 0, complete receipt). The first two
sandboxed runs closed the window but never answered `stopped` within 30 s, and
their guards force-drained and wrote incomplete receipts. They ran under high
host I/O pressure; the cause is unknown and the failures stay recorded.

Later the same day, the coordinator rebuilt the deb and an AppImage and
launched both as an unprivileged user to the home. Packaged actual-App fixture
runs passed: pk3 and pk4 as `nobody` with the sandbox, and pk5 as root. That is
local evidence for this branch's earlier commits, not a qualification of later
sources, of Tauri's AppImage dependency handling (linuxdeploy) on other hosts,
or of a release.

## Windows browser process (first candidate)

`takosumi-cef-browser-process` also builds for `x86_64-pc-windows-msvc`. Until
W6 it did so as an unsandboxed test candidate that must be asked for by name
with the cef-candidate feature `windows-unsandboxed-candidate`; since W7 the
product browser runs in Chromium's sandbox, as the DLL that CEF's
`bootstrap.exe` loads ([Windows sandbox (W7)](#windows-sandbox-w7)), and the
unsandboxed candidate remains for fixtures and diagnostics. This section
describes the candidate as W1 built it. The first Windows step
(W1, this section) ran the browser on its own, started by `windows_smoke.py`
or the CI job. The second (W2, [Windows guard and adapter](#windows-guard-and-adapter))
adds the Windows `takosumi-browser-guard` and lets the App's adapter start
both; nothing is staged or packaged.

```powershell
$env:CEF_PATH = "C:\absolute\owned\cef"   # the first build downloads the Windows distribution here
# From native-tests\cef-candidate: its .cargo\config.toml links the C runtime statically.
Push-Location native-tests\cef-candidate
cargo build --locked --features windows-unsandboxed-candidate --bin takosumi-cef-browser-process --bin takosumi-browser-guard
Pop-Location
python -B native-tests/cef-candidate/windows_smoke.py --browser <target>\debug\takosumi-cef-browser-process.exe --work C:\absolute\owned\cases
```

The standalone `takosumi-cef-candidate` is a Linux experiment and refuses to
compile on other OSes with a single error, and so does the browser without
the feature, instead of building something that does not work as a product
and that a stage could pick up by mistake. The build needs the MSVC
toolchain, CMake and Ninja for cef-dll-sys's `libcef_dll_wrapper`, as
`windows-latest` provides.

The X11 raise (`x11_front`), the euid sandbox decision (`libc` is now a
Linux-only dependency) and the `$ORIGIN` run path are compiled only for Linux,
where the browser's behaviour and output are unchanged; the feature has no
effect there. On Windows:

- **Sandbox.** Off in this candidate, whatever the switches: the executable
  built with `windows-unsandboxed-candidate` has no sandbox information, so it
  sets CEF's `no_sandbox` itself. CEF's Windows sandbox needs the bootstrap
  layout: the browser built as a DLL that CEF's `bootstrap.exe`, renamed to
  the browser, loads, with sandbox information passed to `execute_process`
  and `initialize`. W7 builds exactly that (feature `windows-sandbox`, see
  [Windows sandbox (W7)](#windows-sandbox-w7)); a Windows build with neither
  feature, or with both, is a compile error.
- **DLLs and resources.** cef-dll-sys copies the distribution's files
  (`libcef.dll`, `chrome_elf.dll`, the `.pak`, `icudtl.dat` and V8 snapshot
  files) and `locales/` into the Cargo profile directory beside the executable.
  The Windows loader searches the executable's directory first, and CEF finds
  its resources and locales in that same directory, so no `PATH` or run path is
  needed. A browser started elsewhere needs those files beside it. The
  harness's `--runtime` puts another runtime directory on `PATH` for
  `libcef.dll`; that layout is not exercised on Windows.
- **Manifest.** `build.rs` embeds
  `windows/takosumi-cef-browser-process.exe.manifest` with the MSVC linker
  (`/MANIFEST:EMBED`, `/MANIFESTINPUT`, `/MANIFESTUAC:NO`): the Windows 10/11
  compatibility entry of CEF's cefsimple manifest, without which Windows
  reports an older version to Chromium, plus Common Controls 6 and `asInvoker`.
  It panics for a Windows target that is not MSVC, which would otherwise build
  without the manifest. Until W7 the CI job read the `RT_MANIFEST` resource
  back from this executable with the Windows SDK's
  `mt.exe -inputresource:<exe>;#1` and required exactly that `supportedOS`
  entry, the Common Controls 6 dependency and one `asInvoker` execution level.
  Since W7 the staged executable is CEF's bootstrap, whose manifest CEF
  builds; the job reads that one instead (see W7).
- **Profile directory.** `TAKOSUMI_BROWSER_PROCESS_DIRECTORY` must be absolute
  and in its final form, as on Linux. On Windows the check drops the `\\?\`
  (or `\\?\UNC\`) prefix that `canonicalize` adds and then compares the OS
  strings exactly, so another drive-letter case, `/` separators and repeated
  or trailing separators are refused (`Path` equality would accept them). The
  cache path CEF reports in `opened` is compared the same way. Short (8.3)
  names and links are resolved by `canonicalize`, so the parent passes the
  long final spelling (the harness uses `os.path.realpath`). `boot.json`,
  `Downloads/`, `cef-profiles/<profile>/`, `cef-native.log` and the
  stdin/stdout protocol are unchanged. The Linux HOME/XDG/TMPDIR directories
  have no counterpart; the harness points `TEMP` and `TMP` into its case.
- **Window id and reuse.** CEF's window handle for a Chrome-style browser may
  be a child of its frame. On Windows the `window_id` in `opened` is the
  top-level (`GA_ROOT`) HWND of that frame, the window a person sees and
  switches to and the one an App would focus or pass to Win32 in the next
  step; on Linux it stays the X11 window CEF reports. Reopening a live screen
  raises that same top-level window: it restores it when minimized, shows it,
  moves it to the top of the Z-order (`SetWindowPos` with `HWND_TOP`, since
  W3) and calls `SetForegroundWindow` through minimal `extern "system"` user32
  declarations (the CEF wrapper already links user32). Windows hands the
  foreground only to a process allowed to take it, so an App should call
  `AllowSetForegroundWindow` with the browser's pid before sending the Open
  (the App's adapter does since W3).
  Otherwise Windows flashes the taskbar button and the browser prints
  `Reused browser window was not raised: Windows refused the foreground change`
  to stderr, as it does for an X11 refusal.
- **Console.** Since W5 the executable is a GUI-subsystem program on Windows
  (`windows_subsystem = "windows"` in `process_main.rs`, Windows only): it and
  its CEF helpers, which run the same executable, get no console and so no
  console host, and its standard handles stay the ones its parent passes
  explicitly (the JSON-lines pipes, the log). Until then it kept the console
  subsystem and was started with `CREATE_NO_WINDOW` (W2), so no console
  window opened, but a windowless console host ran with it (see the W5
  finding under the guard).

**Smoke harness.** `windows_smoke.py` uses only the Python 3 standard library
(`ctypes` for Win32) and runs on both OSes. It creates a new case directory
under `--work` (`tempfile.mkdtemp`), lays it out like the App's adapter and
writes the boot Open for `https://example.com/` with a new UUID profile (key
`web-home:<profile>:https://example.com`). It starts the browser directly with
`TAKOSUMI_BROWSER_PROCESS_DIRECTORY` and the App's switches (on Linux also
private HOME/XDG/TMPDIR, `LD_LIBRARY_PATH` and `--no-sandbox` as root). On
Windows the browser starts suspended, is assigned to a Job Object the harness
created with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, and only then resumes, so
every process it starts belongs to the job; on Linux it runs in its own
session and process group.

It requires an `opened` reply for that key and profile, with the expected
cache path and native pid, within 120 s. It then sends
`{"operation":"shutdown","id":2}` and requires a `closed` reply for that
browser, `stopped` for id 2 and exit code 0 within 60 s. Within 10 s of the
exit no process may remain: the job's `ActiveProcesses` (from
`JobObjectBasicAccountingInformation`, reported with `TotalProcesses`) must be
0 on Windows, and the process group must be empty on Linux. `--expect-load` (a
`fixture-inspection` browser) also requires the main-frame load marker, which
shows a renderer ran. `--reuse` sends a second Open of the screen (id 2,
`resume`) and requires the same native browser and window, then requires the
raise: on Windows the reported window must be top level and become the
foreground window (`GetForegroundWindow`) within 2 s, and on both OSes the
browser's stderr must hold no raise refusal. It then shuts down with id 3.

The harness prints a JSON summary and keeps it with `protocol.jsonl`,
`browser.log` and `cef-native.log` in the case, which it never removes. On any
failure it prints the log tails and ends every owned process (terminating the
Job Object on Windows, SIGKILL to the process group on Linux); closing the job
also kills anything still in it.

The CI job `cef-windows` on `windows-latest` built the product browser with
`--locked --features windows-unsandboxed-candidate` (`CEF_PATH` is
`${{ runner.temp }}\cef`, cached by the locked cef-dll-sys version), checked
its embedded manifest and ran the harness, then built the `fixture-inspection`
browser with the same feature and ran it with `--expect-load --reuse`. Since
W7 it builds and stages the sandboxed browser instead and runs both smokes
with `--expect-sandbox` as well (see W7).

**2026-11-06 checks.** The browser type-checks for `x86_64-pc-windows-msvc`
with `cargo check --features cef/dox,windows-unsandboxed-candidate`, and
without the feature it stops at its single compile error, as the guard then did;
`dox` skips cef-dll-sys's build script, which needs the Windows SDK, so
nothing was linked or run for Windows on the Linux host. The Linux build, the
guard self-tests and the plugin tests passed. The first version of the harness
passed once against the Linux `fixture-inspection` browser in a private
`xvfb-run` display with `--expect-load --reuse`; the Job Object, foreground and
leftover checks were added afterwards and were exercised on Linux only with a
stand-in browser. The first CI run (PR #29, head `afd2c44`, before the
feature, the manifest check and the stricter harness) passed both smoke runs
on `windows-latest`, with no raise refusal and no process left by image name.
Windows evidence comes only from the CI job.

## Windows guard and adapter

The second Windows step (W2) makes the App's per-profile browser backend work
on Windows at the adapter level: `takosumi-browser-guard` for Windows, and the
app-screen plugin's `desktop-process-experiment` (and so the App's
`desktop-cef`) compiled for Windows, where it starts the guard and the browser
through the same `Process` code as on Linux. macOS and other OSes still refused
both with a single compile error (macOS builds both since
[M2](#macos-guard-and-adapter-m2)). W2 had no App UI or fixture on Windows
(that came with [W3](#windows-actual-app-w3)), and the browser stays the
unsandboxed candidate.

**Guard** (`src/guard_windows.rs`, chosen by `src/guard_entry.rs`; the Linux
`src/guard_main.rs` is unchanged). Same command line
(`<absolute new receipt> <program> [args...]`), receipt fields and exit codes.
It refuses (exit 2, nothing started) a relative or existing receipt path, a
guard outside any Job Object, and a missing or unopenable stop event. It
creates its own Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` and no
breakaway, reserves the receipt (`CREATE_NEW`, never through a link), and
starts the browser in the job before it runs any code: created
`CREATE_SUSPENDED | CREATE_NO_WINDOW` with the guard's own stdin, stdout and
stderr (the App's control pipe, reply pipe and log), assigned, then resumed.
Since W5 the browser is a GUI-subsystem program, so it and its CEF helpers
have no console and no console host, and `CREATE_NO_WINDOW` has no effect on
it. Until then `CREATE_NO_WINDOW` gave the console browser a console without
a window, with a windowless console host (`conhost.exe`) child in the guard's
job that `observed` counted and the drain had to end (CEF's helpers shared
that one hidden console; started detached, each would have got a console of
its own). It then waits, without polling or a lifetime cap, for the browser's exit or a
stop request. A stop request is the App setting a manual-reset named event
(`Local\takosumi-browser-guard-stop-<App pid>-<runtime>-<nanos>`) that the App
created (an existing name is refused) and named in
`TAKOSUMI_BROWSER_GUARD_STOP_EVENT`; the guard opens it at start and removes
the variable from the browser's environment. Like a signal on Linux, any
process of the same user could set it. The drain always runs: up to 0.5 s for
the browser and every process in the job to end, then `TerminateJobObject`
(the processes still in the job end with the exit code `0xE0007462`, a
customer NTSTATUS no program returns by accident; as `rootExit` it reads
-536841118), then up to 2 s for the job to empty. A job that is found empty
just after the 0.5 s is not terminated, and nothing is counted as forced.
`forcedCleanup` is counted exactly: before terminating, the guard reads the
job's total and then its process list, and holds each listed process open;
afterwards it counts the listed processes that ended with that exit code
(or still run), plus every process that entered the job after the total was
read (counted as forced, so none is missed; one entering between the two
reads may count twice). Since W5 each listed process is classified by one
pure rule (`classify`, a self-test checks its truth table) from the held
handle after the drain's settle: whether the handle is signaled, read first,
then the `GetExitCodeProcess` code. `FORCED_EXIT` is forced; `STILL_ACTIVE`
with the handle unsignaled (still running) is forced; any other code ended by
itself (Windows sets the code before the job drops the process and signals
the handle a little later, so the zero wait on the handle the count used
before read a process that had just ended by itself as still running and
counted it as forced); a failed read is unknown. A `STILL_ACTIVE`,
unsignaled process when the settle found the job empty is given up to 50 ms
to be signaled, never past the end of the drain's existing 2 s forced budget
(so the drain's outer bound is unchanged); with a job not found empty it
counts as forced, a rare over-count on the safe side. A listed process with
any other exit code, or that was already gone when opened
ended by itself and is not counted (an id reused by an unrelated process
between the list and the open can only add a count, never hide one). Any other failure to attribute an end
is recorded in `cleanupErrors` as `ForcedCleanupUnknown`, which makes the
receipt abnormal, as a failed signal does on Linux. The completion proof is
`ActiveProcesses == 0` (`JobObjectBasicAccountingInformation`) after the
browser exited, recorded under the Linux key `kernelNoChildren`; `remaining`
lists the process ids still in the job (`JobObjectBasicProcessIdList`) and
`observed` is `TotalProcesses` (every process ever in the job, the browser
included). A process the browser has started outside its tree through a
system service (COM, WMI, the task scheduler) is not in the job.

**Forced-cleanup diagnostic.** A forced cleanup makes the App keep the
profile, but the receipt does not say what was forced: only renderers
(their state is in memory) or a helper that writes the profile, such as the
network or storage service (an integrity concern). So when the drain reaches
the termination, the guard reads, for each listed process and just before
terminating (a terminated process's command line is gone with its address
space), its image file name (never the path) and its Chromium process type:
only the `--type=` and `--utility-sub-type=` values, from
`NtQueryInformationProcess(ProcessCommandLineInformation)` on the
`PROCESS_QUERY_LIMITED_INFORMATION` handle it already holds (Windows 8.1 and
later). After the count it writes one line to its standard error, the
profile's `browser.log`:
`takosumi-browser-guard: forced-cleanup {"guardPid":G,"forced":N,"processes":[{"pid","image","type","utilitySubType"?,"exit"}],"enteredAfterList":M,"endedByThemselves":K,"identifyMs":T}`,
naming the processes it counted as forced, with each one's exit code after
the termination (`0xE0007462`, or `running`) as the count classified it (see
above), how many listed processes ended by themselves at the boundary, and
how long the identification took. It is written whenever the drain reaches
the termination, also when nothing was forced; `guardPid` names the guard,
since `browser.log` is the profile's and later launches append to it. The
identification runs between the list and the termination, so the effective
natural window is 0.5 s plus `identifyMs`.
`type` is `none` for a process
without one (the browser process itself) and `unknown`,
with the error's name, if the read failed; a value outside
`[A-Za-z0-9._-]{1,64}` reads `invalid`. The full command line never leaves
the reading function (`guard_windows_identity.rs`; the parser,
`process_type.rs`, is unit-tested with quotes, spaces and backslashes). The
receipt keeps the Linux guard's fields, and the budgets and the exit status
are unchanged; the count changed with the classification above (6302b33).
`windows_browser_process.py` collects the lines of each launch's own guards
(by `guardPid`) from every profile's `browser.log` into the launch's
evidence (`execution.json`, `guardRecords`: each guard's receipt and its
`forcedCleanupLines`) and prints them when a launch fails. The Linux guard
does not emit the line.

**Forced cleanup after a normal browser exit (closed by the finding
below).** The real-site run of 483dbe4 (run 42339893587, Photopea, the merged
W4 head)
failed at `quit-partial-order`: Work's browser exited 0 and its guard's
receipt was complete with `observed` 57 but `forcedCleanup` 12, so the App,
as designed, kept the profile ("Browser close remains incomplete: Browser
descendants did not close normally; profile retained") and stopped the
sequential quit before asking Personal. Run 4ed2519 passed the same flow, so
it is intermittent. At the time it was open which processes were forced;
the diagnostic above was added to tell renderers from profile-writing
helpers, and the next run answered it.

**Finding: the forced counts were inflated (run 42343580278, 8044abe).**
With the diagnostic, the next real-site run failed Photopea the same way,
and Work's line named 57 "forced" processes: 52 `renderer` and 4 `unknown`
(NTSTATUS `0xC000010A`, the process was already terminating when read), all
with exit code `0x00000000`, and one `conhost.exe` with `0xE0007462`. So 56
of the 57 had ended by themselves with code 0; they were counted only
because the count read the exit with a zero wait on the handle, which can
still read a process as running just after the job has dropped it (the
self-test of 8044abe showed the same race for the diagnostic's own read).
The only real kill was the browser's console host. The earlier count
(483dbe4: 12 of 57) was likely inflated by the same mechanism (unconfirmed:
that run had no diagnostic). Two repairs:
the count now reads `GetExitCodeProcess` (above), the same read the
diagnostic shows, so the two cannot disagree; and the browser has no console
host to end any more (`takosumi-cef-browser-process` is a Windows
GUI-subsystem program, see the W1 notes). The budgets, the receipt fields
and the App's refusal of any forced cleanup are unchanged: a real kill still
makes the receipt abnormal for the App.

**Parent death.** Windows has no `PR_SET_PDEATHSIG`. The adapter's spawner
thread owns one kill-on-close Job Object for the App and starts every guard
inside it (suspended, assigned, resumed). The guard is started
`DETACHED_PROCESS`: it needs no console, so it has no console host (nor does
the browser, a GUI-subsystem program);
its standard handles are the pipes and log passed explicitly. Jobs nest, so a
browser is in its guard's job and, through it, in the App's. When the App
process ends, however it ends, the system closes the App's job and ends every
guard and browser with it: the profiles are retained, but such guards write
no receipt. The spawner thread drops the job when every requester is gone
(the App state is dropped, or a test ends).

**Adapter** (`process_adapter.rs`, with the Windows pieces in
`process_windows.rs`, `process_setup_windows.rs` and the plugin's
`windows_job.rs`, which the guard also uses):

- The browser and guard are `<resource_dir>\cef\takosumi-cef-browser-process.exe`
  and `takosumi-browser-guard.exe`. Every path given to the browser or
  compared with its replies is canonical in the ordinary spelling
  (`canonicalize` without its `\\?\` prefix), as the browser's own final-path
  check requires.
- The runtime root is `<app cache>\cef`, which is
  `%LOCALAPPDATA%\<identifier>\cef` (Tauri's app cache directory on Windows);
  there is no `XDG_RUNTIME_DIR` candidate. It must be a plain directory (no
  link, junction or other reparse point) whose canonical path lies inside the
  user's profile directory (Tauri's home directory). Not checked: the owner
  SID and access control list of the root (an inherited or explicit grant to
  others is not detected), and whether the profile is on a network share.
  There is no socket-path length rule. Each browser's `p-<pid>-<n>` directory
  is created as on Linux and holds `tmp\` (the browser's `TEMP` and `TMP`) and
  the guard receipt.
- Runtime directory locks: the `lock` file is held open with no read or write
  sharing (only delete sharing), so another App instance's open fails with a
  sharing violation; that is the Windows counterpart of the `flock`. The App
  removes an ended browser's directory while still holding its lock, as on
  Linux (this relies on POSIX delete semantics on NTFS, checked by a CI unit
  test). The sweep and the stale-name rule are unchanged; a lock file that is
  a reparse point is an error, never followed.
- Environment: the guard and browser get only `SystemRoot`, `SystemDrive`,
  `windir`, `ComSpec`, `PATH`, `PATHEXT`, `USERPROFILE`, `LOCALAPPDATA`,
  `APPDATA`, `ProgramData`, `ProgramFiles`, `ProgramFiles(x86)`,
  `CommonProgramFiles`, `ALLUSERSPROFILE`, `PUBLIC`, `USERNAME`,
  `COMPUTERNAME`, `NUMBER_OF_PROCESSORS` and `PROCESSOR_ARCHITECTURE` from the
  App, plus `TEMP`/`TMP`, the stop event and
  `TAKOSUMI_BROWSER_PROCESS_DIRECTORY`; the fixture-only inspection seam stays
  behind `fixture-overrides`. The profile folders are the person's own:
  unlike Linux's private HOME/XDG directories, Chromium on Windows takes its
  data locations from the explicit cache root. No `--no-sandbox` or
  `--password-store` switch is passed.
- Control pipe: Windows has no portable non-blocking write on it, so each
  browser has a writer thread that performs the blocking write while the
  caller waits for it with the same 20 s bound; a write the browser does not
  read in time times out and the next one first waits for it. Closing the
  App's end while a write is pending sets a closing flag the thread reads
  before every write, then cancels the write in progress (`CancelIoEx`),
  repeating every 10 ms until the thread has ended, for at most 2 s, so a
  write the thread issued just after a cancel is cancelled too. The thread
  then writes nothing more of that request. Only a thread that does not get
  to run within those 2 s can outlast the close; that is reported on stderr.
- The `opened` check: the native process is opened first and its handle held,
  so its id cannot name another process during the check. Then Windows must
  record the guard as its parent (a process snapshot taken after opening),
  it must have been created no earlier than the guard (so the record is not
  one left by an ended process whose id the guard reused), its image path
  (`QueryFullProcessImageNameW` on the held handle) must equal the browser
  path exactly, and the reported cache path must equal the requested one
  exactly.
- Stopping a guard that is still running when its `Process` is dropped sets
  the stop event (never terminates the guard, whose job would end the browser
  without a drain or receipt) and waits up to 5 s, as SIGTERM does on Linux.
- `clear_profile`'s "still in use" check opens Chromium's `lockfile` in the
  profile's cache root exclusively: Chromium holds it open for writing, shared
  for reading only, and deletes it on close, so a live browser (tracked by
  this App or not) makes the open fail. A missing file or one nobody holds is
  free; any other failure counts as live.
- An external link the person chose opens with the shell's `open`
  (`ShellExecuteW`) on its own thread, within the same budget of openers.
  This path is compiled into product builds but not exercised by a test; a
  fixture build records the link instead (W3).
- No setuid/setgid identity check exists on Windows; an elevated App is not
  refused.

**CI.** The `cef-windows` job builds the product browser and the guard
together, keeps the W1 manifest check and both smoke runs, and adds two steps.
`Guard self-tests (Windows)` runs `cargo test --bin takosumi-browser-guard`
(thirteen cases in `src/guard_windows_tests.rs`; the self-test binary is its
own stand-in browser, and each case starts the guard detached inside a fresh
kill-on-close job as the App does): a normal end, a non-zero exit, the pipes
reaching the browser without the stop event, a lingering descendant
terminated with the job, a job that empties just as the natural budget ends,
a descendant that ends by itself after the process list was read but before
the termination (not counted as forced, and named among those that ended by
themselves in the diagnostic line; self-test-only injections,
`TAKOSUMI_GUARD_FAULT_LATE_EMPTY` and `TAKOSUMI_GUARD_FAULT_TERMINATE_LATE`,
produce these two orders, which timing alone cannot produce on demand), a
stop request, the App's job ending guard and browser, a pending stop, the
refusals, a browser that cannot start, the forced-cleanup diagnostic's
process type parser, and the forced count's classification truth table (the
lingering case also checks that the diagnostic line names the descendant
killed by the termination).
Cases that require the App's job
to be empty afterwards wait at most 3 s for processes whose exit is already
under way (the stand-ins that must be ended sleep 60 s) and name every
process still there with its image. `App adapter with the built
guard and browser (Process API)` (since W7 `App adapter with the staged guard
and sandboxed browser`) runs the plugin's Windows tests with
`TAKOSUMI_WINDOWS_GUARD` and `TAKOSUMI_WINDOWS_BROWSER` pointing at the
product build (since W7 its stage): unit tests of the pipe writer (a write the reader does not
take times out; dropping the App's end during such a write cancels it, so the
reader sees the end of its input before all the data; that reader is a
Python one-liner, as Python is on the `windows-latest` image; and, with a fake
pipe taking three bytes per write, a request is not continued once closing
is set), the lockfile check
with `clear_profile`'s data handling, the runtime locks and root rules
(including `<app cache>\cef` created when missing), and the unavailable
state; and two `#[ignore]`d tests run with `--include-ignored`.
`windows_adapter_opens_reuses_and_shuts_down_a_profile` starts a profile
through `Process::spawn`, waits for the creation reply through
`expect_open_until` (`expect_open` with a 120 s cold-start bound instead of
20 s; the time is printed), restates the parent, image and cache checks
against Windows' records, holds the browser's direct children open and
requires at least one running CEF helper (running the browser's own
executable) and no console host (`conhost.exe`) anywhere in the guard's
process tree (the guard is detached, the browser and its helpers are a
GUI-subsystem program), and Chromium's `lockfile`
in the profile's cache root held, reopens the same key and requires the same
native browser and window, shuts down through `Process::shutdown` and
requires a normal receipt (complete, `rootExit` 0, no forced cleanup, the
job empty, and `observed` at least the browser plus the helpers held, so a
helper outside the guard's job leaves it short), the
`lockfile` released and the runtime directory's removal.
`windows_adapter_ending_the_app_job_ends_guard_and_browser` starts a
profile, holds a running CEF helper open (a helper that
then ends by itself has still ended), records the guard's process tree,
drops the App's spawner (closing the App's job) and requires every process
held to end within 15 s, naming each survivor with its image, and no normal
receipt. A missing helper prints a fresh view of the browser's tree with
each process's parent and image and the expected program. The step fails
unless each named test passed and exactly fourteen ran (the W6 bundle
resolution test included). The `desktop` job also checks the App with `--features desktop-cef` on
Windows.

**2026-11-06 local checks (W2).** On the Linux host the plugin type-checks for
`x86_64-pc-windows-msvc` with `desktop`, `desktop-process-experiment` and
`fixture-overrides` (library and tests), and the guard and browser with
`cef/dox,windows-unsandboxed-candidate` (the guard also in test mode).
Nothing was linked or run for Windows locally; all Windows evidence comes from
the CI job. On Linux the plugin tests (66 with each experiment feature set),
the 30 guard self-tests, the `fixture-inspection` browser and guard build, a
check of all three cef-candidate bins and a check of the App with
`desktop-cef` passed, with Linux behaviour unchanged.

**First CI runs (W2).** Run 42264960643 (head `5e2250f`) passed the adapter
tests (ten then) but failed the guard self-test
`normal_exit_writes_a_complete_normal_receipt` once: after the guard had
ended, the App's job still counted one process, while the same test had
passed on the run before. The guard was then started with `CREATE_NO_WINDOW`,
so its windowless console host, a child in the App's job, could outlive it
for a moment. The guard is now started detached (no console host), and the
check waits a short, stated bound and names what is left.

**Known W2 limits** (documented, not implemented):

- Between `CreateProcess(CREATE_SUSPENDED)` and `AssignProcessToJobObject`
  the new guard or browser exists outside the intended job. It runs no code
  then, but if its parent dies in that window it stays suspended and
  unowned. `PROC_THREAD_ATTRIBUTE_JOB_LIST` (Windows 10) would create it in
  the job directly; std's `Command` has no stable way to pass it.
- Inheritable handles are not limited to the three standard handles
  (`PROC_THREAD_ATTRIBUTE_HANDLE_LIST` would do that). The App spawns guards
  from one thread, and std serializes its own spawns, but a handle another
  part of the App made inheritable could reach a guard and its browser.
- A browser ended by the drain has `rootExit` -536841118 (`0xE0007462`); a
  program that returned that code itself would look the same, but no program
  returns a customer NTSTATUS by accident. The App only accepts `rootExit` 0
  with no forced cleanup.
- The guard's job check proves that it runs in some Job Object, not that it
  is the App's: Windows 11 and CI runners may place every process in a job.
- The `ShellExecuteW` path for external links is compiled but not exercised
  by a test (fixture builds do not contain it since W3).
- A pending control-pipe write is cancelled when the App's end closes, for
  at most 2 s; a writer thread that does not get to run in that time can
  keep its write blocked until the browser reads or ends (the guard's stop
  request ends a browser that stopped reading). A pending write exists only
  after a write timed out, which can happen before the 20 s bound when an
  earlier write settled close to its own deadline.
- Until W5 `observed` included the browser's console host, so it was one
  higher than on Linux for the same browser; the GUI-subsystem browser has
  none.

## Windows actual App (W3)

The third Windows step (W3) runs the real App on Windows: its home in
WebView2 drives the per-profile CEF path through the App's own adapter, as
the Linux actual-App fixture does, in CI on `windows-latest`. The user flow is
a connection chosen in the home, the site's sign-in stand-in, its app home, an
app opened in that connection's own CEF window, a return to the home through
the OS's window switching (the owner's decision: no in-browser home button),
a reopen that reuses the window, and the App's quit. Nothing is packaged or
signed; the browser stays the unsandboxed candidate.

**Driver** (`src-tauri/tests/desktop/browser_process.rs`, feature
`native-browser-process-fixture`, never enabled by normal builds or
releases). The flow is shared by Linux and Windows and reaches the home
through a small `Shell` (evaluate a script, pause):

- Linux keeps the GTK WebKit view on the GLib main loop
  (`browser_process_linux.rs`); its behaviour is unchanged. gtk, webkit2gtk
  and javascriptcore-rs now come through the feature `native-fixture-gtk`,
  which both actual-App fixtures enable; they are Linux-only optional
  dependencies, so nothing of them is resolved or built for Windows.
- Windows (`browser_process_windows.rs`) uses Tauri's
  `WebviewWindow::eval_with_callback`: WebView2's `ExecuteScript` runs on the
  UI thread and its JSON result comes back. `ExecuteScript` reports no
  exception (a throwing script yields `null`), so each script is wrapped to
  return its value or its error; an error, or a result without a value, fails
  as on Linux. The flow runs on a thread of its own and waits there (30 s per
  script), so the UI thread keeps pumping and the backend's mutex may be taken
  from the flow. No WebView2 COM object is touched directly and no crate is
  added.
- The 90 s watchdog, every checkpoint bound and the markers are Linux's. Only
  on Windows: the first open marker adds the home's top-level HWND
  (`homeWindow`); the reset phase identifies Personal's CEF root by its
  volume serial number and 128-bit file ID (`FileIdInfo`; Linux: its inode)
  and records whether Chromium holds its
  `lockfile` (`personalLockHeld`; Linux records the `SingletonLock` target),
  and the refused clear must leave that lock held; after the home clears
  Work, the App requires Work's `cef-profiles` to be gone and Personal's to
  remain; the phases are `restart` and `reset` (notifications and the native
  file chooser stay Linux-only).
- The first launch prints a progress line per step (as the relaunch phases
  already did), so a launch the runner ends still leaves its last step in
  `app.log`.
- The connection origin is the runner's local site
  (`TAKOSUMI_BROWSER_PROCESS_APP=local` with
  `TAKOSUMI_BROWSER_PROCESS_SITE=https://127.0.0.1:<port>`, checked exactly).
- Tauri resolves the App's data directories on Windows from known folders,
  not the environment, so the App's identifier is
  `com.takosumi.fixture.browserprocess.<case name>`: each case has its own
  App data, home WebView2 profile and browser profiles under
  `%LOCALAPPDATA%` (and `%APPDATA%`), shared by its launches. The runner
  refuses a case name whose data already exists and never removes it.

**External links.** A Windows build with `fixture-overrides` (which the
fixture feature enables) does not contain `ShellExecuteW`: the opener appends
`{"argv":[url],"pid":<App pid>}` to the file named by
`TAKOSUMI_BROWSER_PROCESS_EXTERNAL_LOG` (an absolute path; without it the
link is dropped). The line and the path rule are pure functions in
`process_setup.rs` (`opener_record`, `opener_log`) with Linux unit tests. No
W3 flow chooses to open a link outside (that needs the native context menu,
inspected on Linux only), so the run's case `no-opener-request` shows only
that nothing in these flows handed a link to the opener; the recorder itself
never runs in CI. The product opener remains untested.

**Native occlusion is off on Windows.** The Windows browser adds
`CalculateNativeWinOcclusion` to `--disable-features` (merged with any value
already given) in CEF's `on_before_command_line_processing`, as Electron
does; Linux is unchanged. In run 42306043123 Chromium's native occlusion
tracker marked a browser window covered by the other profile's window
(both at the same rectangle) as hidden, but after the home's reopen had moved
that window to the top of the Z-order (position 10, above Personal at 18 and
the home at 15) and made it the foreground window, neither browser process
recomputed: the reused page stayed `hidden` and the window now below stayed
`visible`. A reused window could therefore stay hidden. The cause inside
Chromium is unknown (it may be specific to the runner's session), and so is
whether the tracker would follow on a real desktop. The trade-off: a covered
window that is not minimized keeps rendering at the normal rate; a minimized
window is still hidden. The reopen's move to the top of the Z-order stays,
since it is right for the real stacking.

**Foreground.** Before it sends a reopen, the adapter now calls
`AllowSetForegroundWindow` with the browser's native pid, so the browser may
raise its window while the person uses the home (a refusal is harmless).
The App reports whether Windows accepted the grant (`foregroundGrant` in the
experiment status, Windows only), and `reuse-raises-window` records it with
Windows' foreground lock before and after the raise and requires the lock
to be active. The runner keeps the lock active (timeout 2147483647 ms), but
that does not show the grant was needed: in run 42312377621 the observer,
which has no grant, took the foreground from the home with a plain
`SetForegroundWindow` under the same lock (in run 42292805130 it had needed
`AttachThreadInput`). Whether the grant is necessary is unproven in CI.

**Browser stop race (fixed in W3).** In run 42292805130 the W2 test
`windows_adapter_opens_reuses_and_shuts_down_a_profile` got `stopped` after a
reuse and then the browser exited with `0xC0000409`, the code of a Rust
abort on Windows; the same test had passed on `13b4e3a`, and the run did not
keep the browser's stderr. The best-supported cause, from the code: the
stdin reader posted requests to the UI thread until the message loop had
returned, while the parent closes its end of the pipe as soon as it reads
`stopped`. Once the raise succeeds (the App now passes the foreground right),
the closing window holds the foreground and Windows hands the activation
over, so the loop returns later and the end of input could be posted in
between. That task then ran during CEF's shutdown, after the browser state
was taken, and `with_state`'s `expect` panicked in a CEF task callback,
which cannot unwind. The reader's gate is now closed before the loop is
asked to quit (and before `stopped` is replied), and a request that still
runs after the state is gone is ignored with a note on stderr. A related
case predates W3: when the person closes the last window (no `shutdown`, so
no `stopped`), a request posted just before the gate closed could still run
before the loop ended, and an Open then raised the pending count and the
exit check panicked (exit 101). The browser now marks itself `quitting` when
the last window has gone and refuses an Open or Close then with the usual
"Stale command or browser stopping" error, while a Shutdown posted just
before is answered `stopped` (once), so the App's quit completes on the
first try; an end of input then closes nothing. The admission rule is a pure
function with unit tests, run on their own locally (the browser binary's
tests need libcef to link, so CI does not run them). The test now prints
`browser.log` (the
browser's stderr, where a panic message lands) and the tail of
`cef-native.log` when the reopen or shutdown fails or the receipt is
abnormal, so a recurrence names its cause.

**Runner** (`native-tests/desktop/windows_browser_process.py`, Python
standard library only; `git` reads the revision). It refuses to run on other
OSes. `--data` names a new case directory inside the user's profile (the
App's runtime root must lie there, so `RUNNER_TEMP` on another drive does not
qualify), named with at most 24 lowercase letters, digits and hyphens: the
name also names the App's data, and Chromium's files under the browser
profiles in `%LOCALAPPDATA%` come close to Windows' 260-character paths.
The layout follows `browser_process.py`: the case holds
`.owned-browser-reference`, `source-snapshot/`, `execution[-phase].json` and
`summary.json`; the session
directory `<case>/<case name>` holds the runtime root `r/`, the App's `tmp/`
(its `TEMP`), `app[-phase].log`, `observer[-phase].log`, the App's markers
and checkpoints, `browser-observation[-phase].json`,
`app-execution[-phase].json` and `external-open.log`. Per launch it:

- serves the fixture site from a thread at `https://127.0.0.1:<ephemeral>`
  for the whole run: `/` is the sign-in stand-in (a name, `work` or
  `personal`, posted to `/session` sets an HttpOnly, Secure, SameSite=Lax
  session cookie; no credential) or, signed in, the app home linking to the
  app page `/app/`. Each page names its role and user in
  `<main data-page data-user>`. Before the first launch it requires the site
  to verify with Python's default context (the Windows ROOT store) and with
  the fixture CA alone;
- starts the App with a filtered environment (no inherited `TAKOSUMI_*` or
  `WEBVIEW2_*`), the fixture overrides for the browser, guard and runtime
  root, the inspection seam and the recording log; created suspended and
  detached from any console, put into a kill-on-close Job Object, then
  resumed, so the App, its WebView2 processes, guards and browsers are all
  in the job;
- runs `windows_browser_observer.py` (85 s), then requires the App's exit 0
  (45 s, as on Linux), the App flow's and the observation's pass, every
  opened browser's normal guard receipt, and the session's counterpart of the
  Linux outer guard receipt: the job empty within 10 s of the App's exit,
  with no forced cleanup. On failure it ends the App, terminates what is left
  in the job (counted as forced), and prints the tails of the App, observer
  and browser logs;
- hashes the executables, `libcef.dll` and the selected sources before and
  after every launch; any change fails the run.

**Observer** (`native-tests/desktop/windows_browser_observer.py`, standard
library only: ctypes for Win32 and a minimal RFC 6455 client for DevTools).
For each profile it requires `inspection.json` to name the attested native
pid, that pid alone to listen on `127.0.0.1:<port>` (`GetExtendedTcpTable`),
the DevTools endpoint on that port, exactly one page target, the fixture page
loaded in a secure context with no App bridge (`__TAURI_INTERNALS__`,
`window.ipc`, WebKit's or WebView2's message handlers), and the reported
`window_id` to be a visible top-level window of that pid. It switches windows
with `SetForegroundWindow` (if Windows refuses, the task switchers'
`AttachThreadInput` route) and never sends keyboard or mouse input. It
cancels a beforeunload dialog through DevTools (`Page.handleJavaScriptDialog`),
where Linux presses Escape, and closes a window with `WM_CLOSE`, where Linux
presses Ctrl+Shift+W. Before the home reopens Work it minimizes Work's
window (`ShowWindowAsync` with `SW_SHOWMINNOACTIVE`, so the home stays in
front). It records each page's visibility and focus with Windows' view of
its window (`IsWindowVisible`, `IsIconic`, placement, rectangle) when it
attaches, with Personal brought to the front, before the reuse and after it,
and the top-level Z-order of the home, Work and Personal around the reuse;
the runner prints this evidence after each launch (and before the log tails
of a failed one). Waiting for a profile to end after its last window
closed (Work before Personal is asked, Personal after its native close)
allows 20 s where Linux allows 10 s; a navigation within the fixture site
may take the cef-candidate popup probe's 25 s; the other bounds are Linux's.

**Cases.** The runner prints `PASS case <name>` for each, in this order:

| Case | What it requires |
| --- | --- |
| `separate-profiles` | Work and Personal (one origin, two connections) open in two guarded browser processes with their own windows; each new profile starts signed out, signs in through the stand-in and opens the app; a nonce in `localStorage`, a script cookie and the HttpOnly session differ per profile |
| `home-by-window-switch` | with Personal's window in front and covering Work (Personal's rectangle contains Work's, Personal above Work in the Z-order), Work's page reports `visible`, so Chromium's native occlusion is really off (when Personal was only now switched to the front, Work's visibility is sampled for 1.5 s and must never read `hidden`); then an OS switch brings the App's own home window (HWND from the App, owned by the App's pid) to the front, visible |
| `reuse-raises-window` | Work's window, minimized from the test side, is iconic and its page `hidden` (a desktop where minimizing does not hide the page fails here); then, with Windows' foreground lock active, the home's tap on Work reuses its browser process, window and documents (no new process or window), the window is restored and in the foreground within 2 s, Work's page is `visible` within 10 s, and the browser logged no raise refusal. Keyboard focus is a known gap, recorded but not required: `document.hasFocus()` and the focus record of Work's GUI thread (`GetGUIThreadInfo`: focus window and its class, active window; it must be captured), with the lock before and after the raise, the App's grant result, the Z-order around the reuse and both pages' visibility are recorded; whether the grant was needed is not shown |
| `quit-cancel-stops` | the App's quit asks Work (oldest) first; its beforeunload opens on the fixture site while Personal is not asked and keeps running; the cancel stops the quit with both browsers and documents kept |
| `quit-partial-order` | on the next quit Work closes with a normal receipt before Personal is asked; Personal's cancel keeps only Personal |
| `quit-completes` | the final quit closes Personal; the App exits 0 with every guard receipt normal and its job empty |
| `restart-keeps-separation` | a relaunch on the same data reopens both from the saved home; each keeps its own storage, script cookie and session |
| `clear-only-one-profile` | a relaunch clears Work from the home: only Work's CEF root is removed and Work is signed out with no storage; clearing the running Personal is refused with its data and `lockfile` kept; after Personal's native close the in-session clear removes it |
| `no-process-left` | every launch ended with its job empty, no forced cleanup and normal guard receipts |
| `no-opener-request` | no link was handed to the App's opener over all launches (the recording log stayed empty) |

**CI.** The job `cef-windows-app` (`windows-latest`, 120 minutes) checks out
the App with mobile-kit and the takosumi contract, builds the home
(`bun run build`) and runs the runner's unit tests
(`native-tests/desktop/test_windows_browser_process.py`: the WebSocket
client, the DevTools bookkeeping, the TCP table parser, the fixture site, the
receipt check and the case list). It names the installed WebView2 Runtime or
stops, builds the `fixture-inspection,windows-unsandboxed-candidate` browser
(since W7 the `fixture-inspection,windows-sandbox` DLL, staged with the
bootstrap by `scripts/stage-windows-cef.mjs --fixture`; see
[Windows sandbox (W7)](#windows-sandbox-w7))
with the guard and the App with `native-browser-process-fixture` (line
tables only), creates the TLS files and trusts their CA
(`native-tests/desktop/windows_fixture_tls.ps1`), runs one case with
`--restart --reset` (20 minutes), and requires each of the ten case names and
exactly ten `PASS case` lines, and a passed `summary.json`. It removes the CA
afterwards whatever happened and keeps the case and the profiles' browser
logs as the artifact `cef-windows-app-case` (14 days). It shares the CEF
distribution cache with `cef-windows`, which it does not change. The fixture
CA is name-constrained to `127.0.0.1/32`, and its thumbprint is recorded
before it enters `LocalMachine\Root`, so the removal always finds it. The
`desktop` job's Linux leg compiles the App with
`native-browser-process-fixture`, so the shared flow and its GTK driver are
built in CI too (the Linux actual-App run itself is local, against real
sites).

**Running it** on a disposable Windows machine, in an elevated PowerShell 7
(the CA goes into `LocalMachine\Root`):

```powershell
$env:CEF_PATH = "C:\absolute\owned\cef"
Push-Location native-tests\cef-candidate   # static C runtime (.cargo\config.toml)
cargo build --locked --features fixture-inspection,windows-sandbox --lib --bin takosumi-browser-guard   # W7: the sandboxed DLL
Pop-Location
node scripts/stage-windows-cef.mjs --fixture --cef "$env:CEF_PATH\154.0.34\cef_windows_x86_64" --bin <target>\debug --out C:\absolute\owned\w3-stage
bun run build
cargo build --locked --manifest-path src-tauri/Cargo.toml --features native-browser-process-fixture
pwsh -File native-tests/desktop/windows_fixture_tls.ps1 -Out C:\absolute\owned\w3-tls
python -B native-tests/desktop/windows_browser_process.py --binary <target>\debug\takosumi-mobile.exe --browser C:\absolute\owned\w3-stage\cef\takosumi-cef-browser-process.exe --guard C:\absolute\owned\w3-stage\cef\takosumi-browser-guard.exe --data "$env:TEMP\sumi-w3-local" --tls-cert C:\absolute\owned\w3-tls\cert.pem --tls-key C:\absolute\owned\w3-tls\key.pem --tls-ca C:\absolute\owned\w3-tls\ca.pem --restart --reset
pwsh -File native-tests/desktop/windows_fixture_tls.ps1 -Out C:\absolute\owned\w3-tls -Remove
```

The case directory and `%LOCALAPPDATA%\com.takosumi.fixture.browserprocess.<case name>`
(and the same under `%APPDATA%` if the App created it) stay behind.

**Known gap: keyboard focus after the reopen (Windows).** After the home
reopens a connection on Windows, its reused window comes to the front and
its page is visible, but in CI the page did not get keyboard focus
(`document.hasFocus()` false). Win32 focus was on the top-level Chrome
widget (`Chrome_WidgetWin_1`, also the active window), and a focus request
the browser posted after the raise did not change it. Chrome's views
windows keep Win32 focus on that widget for toolbar and web contents alike,
so the record cannot tell which has it. Whether keyboard input lands in the
page on a real Windows desktop is unknown and is a real-device check.

**Not proven by W3:** Chromium's sandbox on Windows; packaging (no staged
`cef\`, no MSI/NSIS), Authenticode signing and updates; real devices (only
the CI virtual machine); real sites, a real IdP or popups (the real-site
browser-compatibility runs, downloads, popups, the open-outside choice, the
long alert, notifications and the native file chooser stay Linux-only);
the `ShellExecuteW` opener; native keyboard and mouse input in the browser's
dialogs and windows (the cancel goes through DevTools, the switch through
`SetForegroundWindow`, the minimize through `ShowWindowAsync`, the close
through `WM_CLOSE`); whether the App's foreground grant is needed (its
result is recorded, but the observer took the foreground without one);
Chromium's native occlusion tracking (off on Windows;
whether it would follow reorders on a real desktop is unknown); and other
DPI or multi-monitor layouts.

**First CI run (W3).** Run 42292805130 (head `0b533a8`): `cef-windows-app`
built and passed the first launch up to the reuse (separate profiles signed
in, the home reached by `AttachThreadInput` with the lock active, Work
reused and in front with its owner Work's browser), then failed
`reuse-raises-window` because Work's document reported `hidden`; whether this
desktop ever reports a CEF page as visible was not recorded then. In the
same run `cef-windows` failed the W2 reuse test as described under
"Browser stop race".

**Second CI run (W3).** Run 42298520970 (head `1b21f63`): `cef-windows`
passed again (the stop-race fix: the W2 reuse ended with a normal receipt)
and the Linux GTK fixture passed; `cef-windows-app` failed
`reuse-raises-window` under the visibility rule. Both browser windows opened
at the same rectangle `[10,10,1014,710]`, so Personal (opened second) covered
Work: at attach Work was `hidden` and Personal `visible`, so this desktop does
report visibility (Chromium's native occlusion works there). After the
reuse, Work's window was the foreground window, focused and not
always-on-top, yet its page stayed `hidden`. The raise had only shown the
window and called `SetForegroundWindow`; it now also moves the window to the
top of the Z-order (`SetWindowPos`, `HWND_TOP`, no move or size) before
asking for the foreground, a real reorder that Chromium's occlusion tracker
recomputes on. A refused move is reported like a refused foreground
change. The observer now records the Z-order and Personal's visibility
after the reuse, so a further failure shows whether Work was not reordered
or the reorder was not recomputed.

**Third CI run (W3).** Run 42306043123 (head `bf9a482`): the reorder
happened (Work at Z-order position 10, above Personal at 18 and the home at
15; Work in front; nothing always-on-top), yet Work's page stayed `hidden`
and Personal's `visible`: the occlusion tracker recomputed in neither
process. Native occlusion is now off on Windows (see above), and
`reuse-raises-window` minimizes Work before the reopen and requires the
raise to restore it and make its page visible again.

**Fourth CI run (W3).** Run 42312377621 (head `065b78a`) passed all seven
jobs with the ten cases. Its evidence showed Work restored and in front
after the reuse but its page without keyboard focus: the browser gave focus
before the raise, while the window was still minimized. On Windows it now
asks for focus again after a successful raise, and `reuse-raises-window`
requires the page to report it. The same run showed the observer taking the
foreground without a grant (see "Foreground").

**Fifth CI run (W3).** Run 42316597299 (head `08bae06`): the occlusion-off
check passed (Work, covered by Personal, reported `visible`), but after the
reuse Work, visible and in front, still reported no focus although the
browser had called `set_focus` again right after the raise in the same
task. The cause is unknown: Chrome-style windows may return focus to their
own UI on activation, or the call may come before Windows has processed the
activation. The browser now requests focus from a task posted behind the
raise (no timer), and the observer records the focus record of Work's GUI
thread, so the next run shows whether focus went to the web contents
(`Chrome_RenderWidgetHostHWND`) or to the browser's own UI. Focus stays
required; if the browser's UI keeps it, it becomes a documented gap.

**Sixth CI run (W3).** Run 42319064492 (head `ded5a94`) passed every job
except `cef-windows-app` (the Linux GTK fixture passed this time). Its only
failure was the focus requirement: after the reuse Work's GUI thread had
its focus and active window on Work's top-level window (`Chrome_WidgetWin_1`,
`activeIsWindow` true, `focusInWebContents` false), and `document.hasFocus()`
stayed false with the focus request posted after the raise. Focus is now
the known gap described above, recorded but not required; the posted
request stays.

**2026-11-06 local checks (W3).** Nothing was linked or run for Windows on
the Linux host. For `x86_64-pc-windows-msvc`, the App type-checks and passes
clippy with `native-browser-process-fixture` (C, archive and resource tools
replaced by stand-ins that write empty outputs, since `cargo check` links
nothing), the browser and guard pass clippy (`cef/dox`), and the plugin with
`desktop-process-experiment` and with `fixture-overrides` (library and
tests) adds no clippy warning to the three the current toolchain already
reports in unchanged code; `cargo tree` resolves no gtk for that target. On
Linux the App checks and passes clippy with
`native-browser-process-fixture`, `native-web-home-fixture` and
`desktop-cef`, the browser passes clippy (one existing warning); the plugin
tests pass (68 with each experiment feature set, and the `desktop` library
and capability tests); rustfmt and the source format check pass for the
changed files; and the runner's 17 unit tests pass. The observer's DevTools
client and flows (sign-in stand-in, samples, a beforeunload cancelled
through `Page.handleJavaScriptDialog`) also ran once against a private
headless Chrome and the runner's TLS site on Linux. Windows evidence comes
only from the CI job.

## Windows real sites (W4, manual evidence run)

The fourth Windows step (W4) takes the actual App on Windows to real
third-party sites, as the Linux real-site runs (Excalidraw, Photopea) did. It
is a manual evidence run, not a merge gate: third-party sites change and can
be slow, so it is not part of `ci.yml`.

**What runs.** `windows_browser_process.py --real-site excalidraw` (or
`photopea`) runs the W3 first launch with the fixture home's app link
pointing at `https://excalidraw.com/` (or `https://www.photopea.com/`); there
is no restart or reset phase. Both profiles sign in to the fixture home as in
W3, follow the app link at once, and must reach the site's rendered editor,
which neither a browser error page (another origin) nor the site's own shell
satisfies: Excalidraw's canvas (`.excalidraw canvas`) without its crash
screen (`.ErrorSplash`, which also carries the `.excalidraw` class), or
Photopea's editor start screen ("Open From Computer"). Photopea first serves a
static landing page, shown even if the editor's code never loads, so the
observer enters the editor from it with its "Start using Photopea" button, as
the Linux probe does (`browser-process.mjs`); here the click is a script
`click()`, not CDP pointer input. Both profiles are fresh (this is the first
launch, and the reopen reuses the document), so each must show Photopea's
landing before its editor; an editor without it fails. Each stage (the
document, then from a landing the editor) has 60 s per profile. The site must load in a secure
context with no App bridge, in the profile's one window, and
`separate-profiles` records each profile's site context and how the editor
was entered. Then, on the real pages:

| Case | What it shows on the real site |
| --- | --- |
| `<site>/separate-profiles` | both profiles reach the site through the home; synthetic `localStorage` and script-cookie markers written in each are not seen by the other |
| `<site>/home-by-window-switch` | Work, covered by Personal, stays `visible` (native occlusion off); the home comes back through the OS window switch |
| `<site>/reuse-raises-window` | the minimized Work reopened from the home: the same process, window and document (its markers still there), restored, in front and visible |
| `<site>/quit-cancel-stops` | the App's quit asks Work first; cancelling its (test-added) beforeunload stops the quit with both kept |
| `<site>/quit-partial-order` | Work closes normally before Personal is asked; Personal's cancel keeps only Personal |
| `<site>/quit-completes` | the final quit closes Personal; the App exits 0 with normal guard receipts and an empty job |
| `<site>/no-process-left` | the launch's job ended empty without force |
| `<site>/no-opener-request` | nothing was handed to the App's opener |

On a real site the page or the browser may cancel the test's beforeunload
dialog before the observer does: in run 42334571636 Photopea's dialog
(with a browser-side handler) closed as cancelled (result false, no input)
0.3 s after it opened, in the same second as 23 ad iframes attached and 22
detached, and the observer's own cancel was then told that no dialog was
showing; no guard ended and the quit stopped. The
mechanism is unknown. On real sites that counts as the cancel for the quit
outcome only if the page's own `javascriptDialogClosed` for that dialog has
result false and the profile's guard has not ended; an accept, another
dialog first or an ended guard fails. Each quit case records
`dialogClosedBy` (`observer` or `browser`), and every later check (the quit
stopped, both documents kept, the blocked and cancelled markers, the
partial order, Work closed first) is unchanged. The W3 local-fixture run
still requires the observer's own cancel. Both quit cases' dialogs must
belong to the site's own origin (a cross-origin subframe's would not
count). After a passed run the runner prints, for both,
`DIALOG closedBy=<observer|browser> case=<site>/<case>`; the workflow
requires both lines and emits a warning (not a failure) for `browser`.

A real site's URL is recorded as its origin only (the dialogs, a refused
dialog, a failed stage's document), since a fragment can carry a key (such
as Excalidraw's `#room=` and `#json=`); the artifact is kept for 14 days.
The browsers' own logs (`browser.log`, `cef-native.log`) are kept as they
are.

The W3 invariants hold: the attested owner of each DevTools port, unchanged
source and executable hashes, no opener request. After the final quit
request the observer accepts only a beforeunload that the real site raises on
its own origin, and records it, so the quit can finish; any other dialog type
or origin fails the run unanswered (recorded as `finalQuitDialogRefused`). In
this mode (`TAKOSUMI_BROWSER_PROCESS_REAL_SITE`) the App's first checkpoint
waits up to 450 s, covering the observer's worst case before it (about 370 s:
attach and sign-in at 25 s and each real-site stage at 60 s in both
profiles, plus the window steps; a unit test keeps the two in step), its
watchdog 540 s, and the runner gives the observer up to 530 s.
Anonymous use only: synthetic markers, nothing signs in anywhere real, no
credential is typed.

**Workflow.** `.github/workflows/windows-real-sites.yml` runs only when
asked, and is never a merge gate: dispatched (`workflow_dispatch`, which
GitHub offers once the file is on the default branch) with the
takosumi-mobile ref to test as its input, or by adding the label
`windows-real-sites` to a pull request, which tests that pull request's head
commit (no other pull request event and no push runs it; it never uses
`pull_request_target`, and its token reads contents only). A dispatch runs in
the default branch's context and cache scope, so it tests only this
repository's own commits: it refuses a pull request ref (`refs/pull/...`)
and anything that does not resolve to a commit reachable from one of its
branches, before building, and checks out the commit it resolved. Fork pull
requests: use the label, after review. The concurrency group is the job's
(after its `if`), so a run for another label never enters it and cannot
cancel a pending real run. It builds like
`cef-windows-app` (restoring its caches without saving), trusts the fixture
CA, runs the Excalidraw and the Photopea cases in separate steps (25 minutes
each; the second runs even if the first fails), requires each step's eight
`<site>/<case>` names, exactly eight `PASS case` lines, the two `DIALOG`
lines (warning on a cancel by the page or browser) and a passed
`summary.json`, removes the CA whatever happened and keeps both cases and
the profiles' browser logs as the artifact `windows-real-sites-cases`.

**Not shown by W4:** pointer input reaching a real site (Photopea's start
button is clicked from script; the Linux run used CDP pointer input), the long
native alert,
downloads, popups, notifications and the native file chooser on the real
sites; a real login; and anything W3 does not show (see above).

## Windows packaging (W6)

The sixth Windows step (W6) is the Windows counterpart of the Linux product
path ([Linux packaging](#linux-packaging-desktop-cef), #28): the CEF runtime
and the Rust guard are bundled, the App resolves them without environment
variables, the installer installs it, and the installed App launches
normally. **Boundaries:** the installers are unsigned (no Authenticode, so
Windows SmartScreen would warn) and are not published or uploaded anywhere;
in W6 the browser was still the explicit unsandboxed candidate
(`windows-unsandboxed-candidate`); since W7 both stages hold the sandboxed
browser ([Windows sandbox (W7)](#windows-sandbox-w7)); the CI installers are
debug builds. Authenticode, SmartScreen
reputation, updates and real devices remain operator inputs, as listed in the
control repo's `docs/takosumi/app-release-inputs.md`.

**Layout.** `scripts/stage-windows-cef.mjs --cef <CEF_PATH>/<version>/cef_windows_x86_64 --bin <target>/debug`
copies into `src-tauri/target/windows-cef/cef/`, the same `cef/` layout the
Linux bundle uses: `takosumi-cef-browser-process.exe` (since W7 CEF's
`bootstrap.exe`, with `takosumi-cef-browser-process.dll` beside it),
`takosumi-browser-guard.exe`, `libcef.dll`, `chrome_elf.dll` (which
`libcef.dll` imports), `icudtl.dat`, the `.pak` files, the V8 snapshot, every
`locales/*.pak` (`en-US.pak` required), `CREDITS.html`, and ANGLE
(`libEGL.dll`, `libGLESv2.dll`), the shader compilers (`d3dcompiler_47.dll`,
`dxcompiler.dll`, `dxil.dll`) and SwiftShader (`vk_swiftshader.dll`, its ICD
file, `vulkan-1.dll`) when present. It never stages the import library, the
sandbox library, `bootstrapc.exe` (and, before W7, `bootstrap.exe`) or SDK
build inputs (they are named in the manifest), and stops at any entry it neither stages nor knows
as ignorable (`README.txt`) rather than leave a possible dependency or
resource out. CEF's own BSD license travels with it: the distribution's
`LICENSE.txt` when it has one, otherwise `CREDITS.html` when it names CEF's
copyright holder, otherwise a `CEF-LICENSE.txt` written from the license
comment that opens `include/cef_base.h`; without any of them the stage
stops (the manifest's `cefLicense` says which). Both executables must be
x86-64 PE files, and every staged executable and DLL may import at load
time only Windows system DLLs or files staged beside it; the manifest
records each one's imports. The system DLL list (`scripts/windows-cef-pe.mjs`)
is exactly what the staged CEF 154 runtime, browser and guard import in CI
(run 42355214657): `advapi32`, `cfgmgr32`, `crypt32`, `dwrite`, `gdi32`,
`kernel32`, `ntdll`, `ole32`, `oleaut32`, `rpcrt4`, `user32`, `version`,
`winmm` and `ws2_32`, all on every Windows 10/11 edition (no Media
Foundation, which the N and KN editions lack), plus the `api-ms-win-*` and
`ext-ms-win-*` API sets by prefix (a set newer than the oldest supported
Windows would not be caught). The import tables are read as the loader
maps the image (file data from PointerToRawData rounded down to 0x200,
zeros past SizeOfRawData; an image without an optional header is refused),
and delay-loaded DLLs are not checked. The browser
and guard therefore link the C runtime statically: Cargo builds them from
`native-tests/cef-candidate`, whose `.cargo/config.toml` sets
`+crt-static` for `x86_64-pc-windows-msvc` (like the `/MT` CEF wrapper), so
an installed App needs no Visual C++ redistributable (`VCRUNTIME140.dll`,
which a clean Windows lacks), and a build without it is refused at staging.
In W6 a package stage had to say `--unsandboxed-candidate`, so an
unsandboxed installer was never made by accident; since W7 a package stage
takes the sandboxed browser and `--unsandboxed-candidate` is accepted only
with `--fixture`. A browser with the fixture inspection
seam is refused unless `--fixture`, whose stage
(`src-tauri/target/windows-cef-fixture/`) never goes to the package path. Files are copied, never linked, and `stage.json` beside the stage lists
each one's size and SHA-256. `src-tauri/tauri.windows-cef.conf.json` maps the
stage to the NSIS bundle's `cef/` and installs per user (`installMode:
currentUser`, by default into `%LOCALAPPDATA%\Takosumi`);
`tauri.windows-cef-fixture.conf.json` does the same for the fixture stage,
uncompressed for CI, under its own product name and identifier
(`TakosumiFixture`, `com.takosumi.fixture.package`), so the two installers
never share an uninstall key, a Start menu entry or a default directory
(the fixture App still takes a per-case identifier at run time). The installed tree is `<install dir>\<App>.exe`,
`uninstall.exe` and `cef\`. A release build of the App on Windows is a
GUI-subsystem program (no console window when launched normally); a debug
build keeps its console.

```powershell
Push-Location native-tests\cef-candidate   # static C runtime (.cargo\config.toml)
cargo build --locked --features windows-sandbox --lib --bin takosumi-browser-guard   # W7: the sandboxed browser DLL
Pop-Location
node scripts/stage-windows-cef.mjs --cef "$env:CEF_PATH\154.0.34\cef_windows_x86_64" --bin "$env:CARGO_TARGET_DIR\debug"
bunx tauri build --debug --bundles nsis --features desktop-cef --config src-tauri/tauri.windows-cef.conf.json
```

**Resolution** (`process_setup.rs`, `process_setup_windows.rs`). A product
build (`desktop-cef`) never reads the fixture's overrides; it takes the
browser and guard from `<resource_dir>\cef\`, and Tauri's resource
directory on Windows is the App executable's directory, so an installed App
finds its own installation's `cef\`. Both must be plain files (no link,
junction or other reparse point) whose canonical path lies directly in that
directory, itself a plain directory: a link or junction cannot make anything
outside the installation stand in for them. The same checks run again
before each browser is started (the same canonical paths, still plain files
in the plain directory): links, junctions, moves and deletions after the App
started are refused. An in-place overwrite of a plain file is not detected
(nothing holds the files open), and the moment between that check and the
process creation remains. A fixture override (`fixture-overrides` builds only) only
has to resolve to a file, and an executable left to the bundle beside an
override is still checked. The
runtime root stays `<app cache>\cef` (`%LOCALAPPDATA%\<identifier>\cef`),
a plain directory inside the user's profile, created when missing, as in
W2. If anything does not qualify, the App still starts with the browser
backend unavailable and its reason (as on Linux). A Windows unit test covers
the installed case, a bundle directory that is a junction to files elsewhere,
a missing guard, a mixed override and a file symbolic link.

**CI evidence** (`cef-windows-package`, windows-latest, 150 min). It builds
the product browser and guard and stages them, then the fixture-inspection
browser into the fixture stage. It builds the fixture installer, installs it
silently into a fresh directory (`/S /D=`), requires the installed `cef\` to
hold exactly the staged files (paths, sizes, SHA-256), and runs the W3 cases
against the installed App with `windows_browser_process.py --packaged`: no
browser, guard or runtime override, so the App resolves its bundle and
runtime root itself, the observer requires every guard and browser to run
from the installation's `cef\` (their image paths) and, from a snapshot of
their loaded modules, the browser's `libcef.dll` to come from there and
neither process to have loaded `vcruntime140*` or `msvcp140*` (the static C
runtime at run time), and exactly the ten W3 cases must pass. The fixture
installer carries the product guard: the fixture build rebuilds only the
browser, and the two stage manifests must record the same guard. It uninstalls it silently. It then builds the product
installer (`desktop-cef`), installs it the same way, runs the installed
product browser alone through `windows_smoke.py` (boot open, shutdown,
`stopped`, exit 0, nothing left in its job), and launches the installed App
once (`windows_package.py smoke`): on a runner where it has no runtime root
yet, with no `TAKOSUMI_` or `WEBVIEW2_` variable, inside a kill-on-close
job, its window must appear, it must stay up for 8 s, must not report its
browser unavailable and must have created its runtime root; `WM_CLOSE` must
end it with exit code 0 and the job must empty. Each uninstall first
requires the per-user uninstall key and the Start menu shortcut of the
productName its installer was built with (read from its Tauri config) to
exist, so their absence afterwards is the uninstaller's doing; it must then
remove the installation directory, that key and that shortcut and leave no
process running from it (run 42351529951 read the key
and shortcut right after the directory went, before the uninstaller had
removed them; the check now waits for all three). What remains is reported, not removed: the App's data
directories (`%LOCALAPPDATA%\com.takosumi` with its WebView2 data, runtime
root and browser profiles, and the fixture's per-case
`com.takosumi.fixture.browserprocess.*`, plus `%APPDATA%` entries if any). The
artifact keeps reports, logs, stage manifests and packaged-case metadata in
one directory. Browser profile directories are excluded, including database
files named `.log`. The standalone smoke copier accepts only `boot.json`,
`browser.log`, `cef-native.log` and `summary.json` directly under an owned
smoke case: plain, single-link files, at most 8 MiB each, valid UTF-8 and no
NUL bytes. It copies the inspected bytes and refuses links and a reused output
directory. The general report copier permits `.json`, `.log`, `.txt`, `.html`
and `.md`, rejects executables, libraries and archives by name or first bytes
(MZ, ELF, ZIP), and logs its rejected files and copy errors. Profile-level logs
are limited to the two native diagnostic names, without descending into site data.

**Not shown by W6:** a signed or release installer, SmartScreen, an update
over a running App, a machine-wide install, real devices, the Windows
sandbox (W7 adds it), and opening a connection in the installed product App
(the packaged fixture App does that; the product App is only launched and
closed).

## Windows sandbox (W7)

Status: implemented on 2026-11-07; the Windows evidence is the CI run of this
change (nothing was linked or run for Windows on the Linux host). The seventh
Windows step runs the per-profile browser in Chromium's sandbox, so the
product path no longer needs `windows-unsandboxed-candidate` (on Linux, #28
already runs the sandbox for non-root users).

**Route.** Since CEF M138 a Windows embedder gets no `cef_sandbox.lib` (it
links only with Chromium's own Clang and libc++); CEF 154's distribution has
the prebuilt `bootstrap.exe` (GUI subsystem) and `bootstrapc.exe` (console)
instead. Renamed to the client, the bootstrap starts every process (the
browser and each subprocess, which run the same executable), sets Chromium's
sandbox up (the broker in the browser process, the target side in sandboxed
subprocesses), loads `<its own name>.dll` from its own directory and calls
that DLL's exported `RunWinMain(HINSTANCE, LPWSTR, int, void *sandbox_info,
cef_version_info_t *)`. The locked crates support that as they are: cef
154.5.0's `execute_process` and `initialize` take the sandbox information
pointer, its `sandbox` feature builds the wrapper with `USE_SANDBOX=ON`
(cef-dll-sys's `/MT` wrapper, matching the static C runtime), and cef-rs's
own cefsimple example uses the same layout. Nothing is forked or vendored.

**Build.** The cef-candidate feature `windows-sandbox` (`cef/sandbox`) builds
the crate's library target `takosumi_cef_browser` (`src/browser_dll.rs`, a
cdylib): it exports `RunWinMain`, which passes `sandbox_info` to
`process_main::run`, which hands it to `execute_process` and `initialize`,
with CEF's `no_sandbox` left at 0. The library is empty on every other target
and without the feature, so Linux builds the same browser as before from the
new executable root `src/browser_main.rs`. The executable built with
`windows-sandbox` refuses to run (exit 2: it would have no sandbox
information). A Windows browser built with neither feature, or with both,
stops at a compile error. `windows-unsandboxed-candidate` stays for fixtures
and diagnostics: its executable can be staged only with
`--fixture --unsandboxed-candidate`.

```powershell
Push-Location native-tests\cef-candidate   # static C runtime (.cargo\config.toml)
cargo build --locked --features windows-sandbox --lib --bin takosumi-browser-guard
Pop-Location
node scripts/stage-windows-cef.mjs --cef "$env:CEF_PATH\154.0.34\cef_windows_x86_64" --bin "$env:CARGO_TARGET_DIR\debug"
# A fixture: --features fixture-inspection,windows-sandbox --lib, then the stage with --fixture.
```

**Stage.** The product stage copies the distribution's `bootstrap.exe` as
`takosumi-cef-browser-process.exe` and the built `takosumi_cef_browser.dll` as
`takosumi-cef-browser-process.dll`, beside the guard and the CEF runtime of
W6. Both go through the same PE checks (x86-64; load-time imports only from
the system DLL list or files staged beside them): a bootstrap import outside
the list stops the stage, and the list is then changed deliberately. The
fixture inspection seam is looked for in the DLL, which holds the browser's
code. `bootstrapc.exe` stays unstaged. `stage.json` records which browser it
holds (`sandbox`, `unsandboxedCandidate`). The App is unchanged: it starts
`cef\takosumi-cef-browser-process.exe` (now the bootstrap) through the guard,
and the W3 runner also hashes the DLL beside a browser it is given.

**Constraints.**

- **Signing.** In the browser process, before it runs the client, CEF 154's
  bootstrap checks three files (`libcef_dll/bootstrap/bootstrap_win.cc` at
  CEF commit `14c5a08`, the locked 154.0.34: `CheckDllCodeSigning`, lines
  164-193, and its callers, lines 918-993). The executable itself must be
  unsigned or carry only valid signatures. `chrome_elf.dll`, which it
  preloads, must lie in the executable's own directory. Then `chrome_elf.dll`
  and the client DLL must each be unsigned when the executable is, or carry
  only valid signatures with the executable's primary certificate thumbprint
  (`ThumbprintsInfo::IsSame` with unsigned allowed); any mismatch is fatal.
  Subprocesses skip these checks (the browser process made them). So
  `takosumi-cef-browser-process.exe` (the bootstrap),
  `takosumi-cef-browser-process.dll` and `chrome_elf.dll` must be all
  unsigned or all signed with the same certificate; CI runs them all
  unsigned, as CEF distributes them. The policy for a signed release: sign
  every PE file under `cef\` (the bootstrap, the client DLL, `libcef.dll`,
  `chrome_elf.dll`, the ANGLE, shader compiler and SwiftShader DLLs, the
  guard) and the App with the same certificate. Signing changes their bytes,
  so it comes before the stage manifest and the installer are made (or the
  manifest is written again).
- CEF 154's bootstrap also carries CEF's installer logic, run by
  `/cef-update`, `/cef-uninstall` and related switches or by an installer
  configuration loaded from the client DLL. The client DLL holds none and the
  App passes no such switch; this was not examined further.
- The DLL must keep the executable's base name and sit beside it; renaming the
  browser executable means renaming its DLL with it.
- The executable is CEF's prebuilt binary from the locked distribution, so it
  moves with cef-dll-sys's version; its embedded manifest, icon and version
  resources are CEF's (build.rs's manifest applies only to the unsandboxed
  candidate). Changing those resources would need a resource editor and must
  happen before signing.
- The browser process itself runs at its caller's integrity level (medium
  for a standard user; high on the elevated CI runner, as the W1 evidence
  shows); the sandbox restricts its subprocesses, renderers to a restricted
  token at Untrusted or Low integrity. In the App each browser runs in its
  guard's Job Object inside the App's, and Chromium puts its sandboxed
  processes in jobs of their own below those (nested jobs, Windows 8 and
  later). The checks read each renderer's integrity level and command line,
  in W1 and the installed-browser smoke (the smoke's own job holds the
  browser) and in the App's runs (W3, the W6 packaged run, W4: the guard's
  job inside the App's; see [In the App](#in-the-app-w7)); they do not read
  the jobs' limits, the desktop or the token's restricting SIDs.

**CI evidence.** Runs on this branch: at `c67a29c`, every Windows job of
ci.yml passed (run 42363489107; the `desktop` job's Ubuntu leg hit the known
WebKitGTK failure) and the real sites passed (42363524297). At `0761843`,
ci.yml passed 8/8 (42369536894); in the real sites (42369567617)
Excalidraw passed 8/8 with the in-App sandbox check, and Photopea's partial
quit failed (an open RED, below). `cef-windows` builds the
DLL and the guard, stages them (`--out $RUNNER_TEMP\w7-product`), requires
the staged bootstrap's embedded manifest to name Windows 10/11 among its
`supportedOS` entries and to ask for no elevation (`asInvoker`, or no level,
which an x64 image treats the same), warns when it has no Common Controls 6
dependency (CEF's binary, not ours), and runs
`windows_smoke.py --expect-sandbox` against the staged executable, the guard
self-tests, and the App's adapter tests with the staged browser and guard;
then the `fixture-inspection` DLL, staged with `--fixture`, runs with
`--expect-load --reuse --expect-sandbox`. The smoke summaries, their logs
and the product `stage.json` are kept as the artifact `cef-windows-smoke`.
`cef-windows-app` (W3) and `windows-real-sites.yml` (W4) stage the sandboxed
fixture browser and hand the App those files; `cef-windows-package` (W6)
stages the sandboxed browser in both installers, its packaged W3 run's
observer also requires each installed browser (the bootstrap) to have loaded
`takosumi-cef-browser-process.dll` from the installation's `cef\`, and it runs
the installed product browser with `--expect-sandbox`.

**`--expect-sandbox`** (`windows_smoke.py`). Before the shutdown it lists the
job's processes (`JobObjectBasicProcessIdList`), reads each one's command
line (`NtQueryInformationProcess`, `ProcessCommandLineInformation`, with
`PROCESS_QUERY_LIMITED_INFORMATION`) and token integrity level
(`OpenProcessToken`, `GetTokenInformation(TokenIntegrityLevel)`), and waits
up to the open timeout for a `--type=renderer` process. It then requires the
browser's integrity level and command line to be readable, without
`--no-sandbox`, at least
one renderer, every renderer to carry no `--no-sandbox` and to run at
Untrusted (0x0000) or Low (0x1000) integrity, below the browser's, and every
other process whose command line could not be read (so whose type is
unknown) to have already exited (its id no longer opens,
`ERROR_INVALID_PARAMETER`, or it has an exit code) or to end within 1 s: a
process being terminated can lose its readable command line while its exit
code still reads `STILL_ACTIVE`, so it is reopened with `SYNCHRONIZE` and
waited for before it counts as a problem (`endedAfterWait` when it ended).
The summary records it as `sandbox` (each process's pid, integrity level
and the process types seen, and the processes whose command line could not
be read, with the reason and whether they had exited); any problem fails
the run. Command
lines are split like the C runtime (unit-tested). The c67a29c run's W1
evidence: browser high, every renderer untrusted, no `--no-sandbox`, no
unread process.

<a id="in-the-app-w7"></a>**In the App** (`windows_browser_observer.py`, so
W3, the W6 packaged run and W4). Once each profile's page has loaded, the
observer takes that profile's browser and its descendants by parent process
id (a Toolhelp process snapshot: the browser runs in its guard's job inside
the App's, so no job holds just it; a process created before the one its
parent id names is not its child, so a reused id adds nothing) and applies
the same evidence function (`sandbox_evidence`), with the same
requirements: at least one renderer per profile, each Untrusted or Low
below the browser's, no `--no-sandbox`, no live unread process. A real-site
run (W4) checks each profile again once the real site is ready, so the
site's own renderers (site isolation, ad frames) are covered too. Each
record goes into the launch's `browser-observation*.json` (`sandbox`, with
its `stage`: `attach` or `real-site`) and the CI log's evidence line; the
runner checks again that every opened profile of the launch has a record
from each stage (both in a real-site run) without a problem and keeps a
summary (stage, the browser's and renderers' integrity levels) in each
phase of `summary.json`. Any problem fails the
launch. `--unsandboxed-candidate` (runner and observer) records without
requiring, for diagnostics with the unsandboxed candidate only; CI never
passes it.

**Shown on the Linux host (2026-11-07):** the Windows-target clippy of the
browser (the sandboxed DLL, product and fixture, and the unsandboxed
candidate), the guard, the App (product and fixture) and the plugin, with
`cef/dox` (cef-dll-sys's build script, which needs the Windows SDK, skipped);
the Linux browser build; the stage tests (both layouts, a missing DLL or
bootstrap, a DLL without `RunWinMain`, the export table reader); the runner,
observer and smoke unit tests.

**Exit codes.** `process_main::run` returns the process's exit code (a CEF
helper's from `execute_process`, the browser's 0, or 1 when a request
failed). A CEF helper's `RunWinMain` returns it to the bootstrap, which
returns it from its `wWinMain`. The sandboxed browser process does not come
back: once CefShutdown has returned it ends itself with `ExitProcess` and
that code (below). A panic reaching `RunWinMain` is caught; the process
then flushes its standard output and error and ends with
`ExitProcess(101)` (Rust's code for a panicking `main`), so a panic after
the message loop does not run the broker's teardown and turn into 7014.
Only panics on `run`'s own path get there: a panic inside a CEF callback
(an `extern "C"` function) cannot unwind and aborts the process with
0xC0000409 instead, before anything reaches `RunWinMain`. The executable (Linux, the unsandboxed candidate) exits with a
non-zero code and returns from `main` on 0, as before. The stage refuses a
client DLL that does not export `RunWinMain` (a library built without
`windows-sandbox` is empty). The crate declares `rust-version = "1.89"` (its
path dependency, the App's plugin, needs it).

**How the browser ends after CefShutdown** (rounds 3 to 5). Runs of
Photopea's sequential quit (W4), whose page leaves dozens of sandboxed
targets (site-isolated ad frames, each a renderer, and utilities):

- `c67a29c` (run 42363524297): the browser ended with `std::process::exit`
  (`ExitProcess`) after CefShutdown, and so did every CEF helper process
  after `execute_process`; Photopea passed 16/16.
- `0f6fa36` (run 42366574526): the browser returned its code to the
  bootstrap (round 2). Work's guard receipt had `rootExit` 7014 (`complete`,
  nothing forced, 92 processes observed), so the App stopped the quit as an
  abnormal end before asking Personal.
- `7e90bfb` (run 42368077968): before returning, the browser waited for its
  children: `{"children":65,"remaining":0,"waitedMs":7548,"boundMs":10000,"exitCode":0}`,
  receipt `rootExit` 0, nothing forced, 77 observed; no 7014, but Work's end
  then took so long that Personal's beforeunload did not open within the
  observer's 20 s (`END_WAIT`), which stays as it is.
- `0761843` (run 42369567617), the browser ending with `ExitProcess` after
  CefShutdown (below): **an open RED, cause UNKNOWN.** Work ended as
  designed (`end` `exit-process`, exit code 0, 46 children still running at
  the end; guard receipt `rootExit` 0, nothing forced, 76 observed), but its
  page frames detached only about 25 s after the partial quit was
  requested, with no dialog, so Personal's beforeunload did not open within
  `END_WAIT` (20 s). Work's CEF log shows page activity until about
  11:49:40 and the cookie flush at 11:49:42, then the shutdown line. Whether
  the time went to the page's close, to CefShutdown or to the guard's drain
  is not known; the `closeMs`, `shutdownMs` and `totalMs` the line now
  carries (below) split it on a recurrence. Excalidraw passed 8/8 in the
  same run, and the real-site workflow is evidence, not a merge gate.

From Chromium 154.0.8037.98's sources (the locked CEF 154.0.34):

- Returning to the bootstrap runs the sandbox broker's teardown. The broker
  lives in the bootstrap (`content::InitializeSandboxInfo` in
  `bootstrap_win.cc`) as a `SingletonBase`, whose `_onexit` handler deletes
  it once `wWinMain` returns (`sandbox/win/src/win_utils.h`);
  `ExitProcess` skips an executable's `_onexit` handlers.
  `BrokerServicesBase::~BrokerServicesBase` (`broker_services.cc`) posts
  `THREAD_CTRL_QUIT` to its job-event thread and, if that thread has not
  ended within 5000 ms, calls `TerminateProcess(GetCurrentProcess(),
  SBOX_FATAL_BROKER_SHUTDOWN_HUNG)`, 7014, "Broker terminated in shutdown"
  (`sandbox_types.h`). On the quit the thread destroys every job tracker
  still listed (one per target whose job has not reported
  `JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO`), one after another: `~JobTracker`
  terminates the job and `~TargetProcess` waits up to 50 ms for the process,
  terminates it and resets its IPC server, which unregisters its waits with
  `UnregisterWaitEx(..., INVALID_HANDLE_VALUE)` and so blocks on any
  callback in flight. CefShutdown does not wait for its children: Chromium
  terminates renderers with `Process::Terminate(RESULT_CODE_NORMAL_EXIT,
  false)` (`child_process_launcher_helper_win.cc`) and deletes the other
  child hosts (`BrowserChildProcessHostImpl::TerminateAll`). So on a heavy
  page that ordered cleanup takes seconds: past the 5 s (7014), and waiting
  for the children first took 7.5 s.
- Every target's job ends with the broker: `BrokerServicesBase` creates a
  job for each target (`PolicyBase::InitJob`, from `PreSpawnTarget` on the
  way to `SpawnTargetAsync`), and
  `Job::Init` (`sandbox/win/src/job.cc`) sets
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` for every job level (`kLockdown`,
  `kLimitedUser` and `kInteractive` fall through to `kUnprotected`, which
  sets it). The broker holds the job handles; when its process ends, the
  kernel closes them and ends the targets still in those jobs.
- CefShutdown has already shut the browser down and asked the profile to
  flush before it returns. The network service, which holds the cookies, is
  a separate (utility) process in a job of its own, ended through that job
  like the other targets. That data survives is shown by W3's restart case
  (each profile's storage, cookie and session after an App restart), not
  guaranteed by this reasoning.

So once CefShutdown has returned, the sandboxed browser ends with
`ExitProcess` and the run's exit code (`src/children_windows.rs`, Windows
only), and the kernel ends the remaining targets through their jobs; the
guard then sees them end. That is the browser process's end at `c67a29c`;
unlike then, CEF helper processes now return their code to the bootstrap
(round 2), where they have no broker to tear down. There is no wait: just before
ending, the browser counts its child processes still running (a Toolhelp
snapshot; a child is a process whose parent id is its own and that was
created after it; no wait) and writes one line to its standard error (the
guard's `browser.log`):
`takosumi-cef-browser-process: shutdown {"pid":…,"children":…,"unopened":…,"end":"exit-process","exitCode":…,"closeMs":…,"shutdownMs":…,"totalMs":…}`
(`unopened`: children it could not open to tell; `closeMs`: from the App's
latest Close or Shutdown request (or the end of its input) reaching the UI
thread to the last window's `on_before_close`; `shutdownMs`: from the quit
of the message loop to CefShutdown's return; `totalMs`: from that request
to the line; `null` when a moment is missing). With the guard receipt and
the App's own log this splits a slow end into the page's close, CefShutdown
and the guard's drain. A failed write of the line is ignored. The unsandboxed candidate
writes the same line with `"end":"return"` and returns its code as before;
Linux is unchanged. Unsandboxed child processes, if any, are not in a
sandbox job; they end on their own once their connection to the browser is
gone, and the guard's drain covers the rest as before. No bound of the App,
the guard or the observer changed, nor the App's policy (a non-zero
`rootExit` is abnormal). The W3 runner keeps the line per profile in each
launch's `execution*.json` (`guardRecords[].browserShutdown`), in each
phase of `summary.json` (`shutdown`, with the guard receipt's `rootExit`)
and prints it to the CI log (`shutdown-first`, `shutdown-restart`, ...); the
W1 smoke records it in its summary (`shutdown`). The CI job `cef-windows`
builds the module's tests with `rustc` alone and runs all six on Windows:
the pure parts (children by parent id, the line, the durations), and the
count of the test binary's own children (it re-executes itself as them),
running and ended.

**Not shown:** Authenticode signing (every PE file under `cef\` with the
same certificate), SmartScreen, a release build, real devices and other
Windows editions or ARM64 (the stage is x86-64 only); the sandbox of the GPU,
network and utility processes (only renderers are checked); the job,
desktop and restricting-SID restrictions individually (integrity level and
command line only); AppContainer or LPAC renderers.

## macOS browser process (M1)

Status: implemented on 2026-11-07. CI at `a8e4b08` (run 42380247099) passed
9/9: both `cef-macos` smokes passed, every renderer was in the seatbelt
sandbox and the browser process was not (by design). This Linux host cannot build or link for macOS: the
browser, the helper and the App were type-checked and linted for
`aarch64-apple-darwin` here (`cargo clippy` with `cef/dox`, which skips
cef-dll-sys's build script; no Apple SDK is needed for that), nothing was
linked or run. The first macOS step mirrors W1: the per-profile browser
builds and runs on macOS on its own, started by the smoke harness.

**How CEF 154 runs on macOS** (CEF's GeneralUsage "macOS", and what the
locked cef 154.5.0 and cef-dll-sys crates provide):

- **Bundle.** The browser is an app bundle: `Contents/MacOS/<name>` (the
  browser process), `Contents/Frameworks/Chromium Embedded Framework.framework`
  (CEF itself, with its resources, locales and `Libraries/`), and the helper
  apps in `Contents/Frameworks`. cef-dll-sys downloads the macOS distribution
  (`cef_binary_<version>_macosarm64_minimal`) into `CEF_PATH/<version>/cef_macos_aarch64`
  (its Release directory, holding the framework, plus `include`, `libcef_dll`
  and `cmake`) and builds `libcef_dll_wrapper`; on macOS it links only AppKit
  and the wrapper, not the framework, and copies nothing beside the
  executable: assembling the bundle is the embedder's job.
- **Helpers.** CEF starts every subprocess from a helper app named after the
  browser executable: `<name> Helper.app` (utilities, such as the network
  service), `<name> Helper (GPU).app`, `(Renderer)`, `(Plugin)` and
  `(Alerts)`, each `Contents/MacOS/<name> Helper…`, an `LSUIElement` app
  without a Dock icon. The browser executable itself never runs a
  subprocess on macOS.
- **Library loader.** The browser and each helper load the framework
  themselves before any CEF call (`cef_load_library`, CefScopedLibraryLoader
  in C++): the wrapper's macOS loader (`libcef_dll_dylib.cc`) resolves every
  CEF function from the loaded framework. cef-rs's
  `cef::library_loader::LibraryLoader` does it from `../Frameworks` (the
  browser) or `../../..` (a helper, from inside its own app).
- **NSApplication.** The browser process's shared `NSApplication` must
  implement CEF's `CefAppProtocol` (`isHandlingSendEvent` /
  `setHandlingSendEvent:`) and exist before CefInitialize; cef-rs provides
  the protocols (`cef::application_mac`), its cefsimple example the
  subclass.
- **Info.plist.** `CFBundleExecutable`, `CFBundleIdentifier`,
  `CFBundlePackageType` `APPL`, the version keys, `LSMinimumSystemVersion`,
  `LSEnvironment` with `MallocNanoZone` `0` (as CEF asks),
  `NSHighResolutionCapable`, `NSSupportsAutomaticGraphicsSwitching`, the
  usage descriptions macOS requires before a page may ask for the camera,
  microphone, Bluetooth or a passkey, and `LSUIElement` for the helpers.
- **Sandbox.** The helpers run in the macOS seatbelt sandbox, which each
  helper enters itself before it loads the framework: CEF 154 ships it as
  `Libraries/libcef_sandbox.dylib` inside the framework
  (`cef_sandbox_initialize`, CefScopedSandboxContext in C++; a process type
  that is not sandboxed returns at once); cef-rs's `cef::sandbox::Sandbox`
  loads it from the helper's place in the bundle. The browser process is
  not sandboxed.
- **cef-rs's tooling.** `cef::build_util::mac` and its `bundle-cef-app`
  binary build such a bundle through `cargo build`; they need cef's
  `build-util` feature (with cargo_metadata, plist, clap), which this crate
  does not enable (`default-features = false`). Nothing blocked the route
  without forking the crates.

**Route.** The same browser source (`process_main.rs`) builds for macOS, and
the bundle is assembled by `scripts/bundle-macos-cef.mjs`:

- `browser_main.rs` on macOS loads the framework from the bundle (exit 2,
  with the reason, when the executable is not in one), installs
  `TakosumiCefApplication` (`mac_front.rs`: the `NSApplication` subclass with
  CEF's protocols, the regular activation policy) and runs the browser. A
  quit from the system (`terminate:`) sends a profile-bound `quitRequested`
  intent to the parent App. Its reply reader consumes the intent separately
  from command replies and enters the App's ordinary sequential exit path.
  The parent retains nonzero Shutdown IDs, beforeunload cancellation and the
  ability to reopen a retained profile; the child does not call Cocoa exit
  or overwrite an in-flight Shutdown ID. objc2 adds a protocol to the class only when the
  runtime knows it and skips a missing one silently, so
  `install_application` requires `CrAppProtocol` and `CrAppControlProtocol`
  to exist and the class to conform to both, and takes `sharedApplication`
  as a plain `NSApplication` before checking it is this class (it panics,
  naming the reason, otherwise). `sendEvent:` restores the handling flag
  with a drop guard (also when an Objective-C exception unwinds through
  it), and the raise holds the NSWindow retained while it sends it
  messages.

  The M3 fixture's first two quit requests use AppKit's
  `NSRunningApplication.terminate()` on the already attested browser PID.
  A fixture-only helper rechecks browser/guard PID, parent and birth before
  sending the normal request; the observer also rechecks the pinned images.
  Request acceptance is not completion: real beforeunload cancellation,
  retained profiles/reopen and normal guard receipts remain required. The
  final quit still comes from the Tauri host. This covers the OS quit API,
  not physical Cmd-Q/Dock/menu input, and does not fix or mask the separate
  Chromium teardown-watchdog failure. The API has no atomic PID/birth form;
  the small race after the last identity check remains a fixture limitation.
- `helper_main.rs` (new executable `takosumi-cef-browser-helper`): enters the
  seatbelt sandbox, loads the framework and runs the process type CEF asked
  for, as CEF's own helper and cef-rs's cefsimple helper do. No application
  code runs in a helper. On every other OS it refuses to run (exit 2).
- `process_main.rs` on macOS: the cache path CEF reports and the profile
  directory compare exactly (as on Windows; the parent passes the final
  path, `/private/var/…` rather than the `/var` link); the window id in
  `opened` is the `windowNumber` of the NSWindow holding CEF's view (its
  window handle is that window's content view);
  reopening a live screen raises that window (`mac_front::raise`: restore
  when minimized, `activate` on macOS 14 and later or
  `activateIgnoringOtherApps:`, `makeKeyAndOrderFront:`,
  `orderFrontRegardless`); CEF's own sandbox setting is on unless the
  parent passes `--no-sandbox`. Linux and Windows are unchanged.
- **Waiting for the window.** The first CI run (`e2c6bd9`, run 42378830347)
  built, downloaded CEF and bundled (238 entries, the sandbox library
  present), but both smokes stopped at "The opened reply has no native
  browser window": `opened` carried window id 0. A Chrome-style window is
  created and shown after `on_after_created`, so CEF's view may not be in an
  NSWindow yet (or its handle not set) at that point; which of the two
  happened is unknown. The browser now logs, at `on_after_created`, whether
  the window handle is null, its class, whether it is in an NSWindow and the
  window number (`macOS: browser <id> created: …` on standard error), and
  on macOS sends `opened` only once the window is there, looking again every
  25 ms on the UI thread (`post_delayed_task`); it logs when it appeared
  (`… window after <ms> ms: …`). At `a8e4b08` the log showed the handle null
  at `on_after_created` and the window (a `BridgedContentView` in an
  NSWindow) 181 and 244 ms later. Until then the screen counts as opening,
  as before its creation (a Shutdown is answered CloseBlocked). Every end
  of the wait without `opened` answers the Open with an error and stops
  counting the screen as opening (`src/window_wait.rs` decides, pure, with
  tests that `cef-macos` runs): the screen closed meanwhile (also through
  the end of the App's input, whose close_all closes every screen,
  this one included); the browser is stopping (the screen is closed; the
  refusal every late request gets); or no window within 15 s (the screen
  is closed). 15 s is shorter than the adapter's 20 s wait for an Open
  reply, so the App gets that answer rather than a timeout that would leave
  the browser in doubt, and about 60 times what the window took in CI. A
  look that cannot be posted ends the wait the same way. The raise resolves
  the window again each time.
- `scripts/bundle-macos-cef.mjs --cef <cef_macos_aarch64> --bin <target>/debug --out <dir> [--fixture]`
  writes `<dir>/takosumi-cef-browser-process.app` (the layout above; each
  helper app holds a copy of the helper executable under its own name; the
  framework is copied with its symbolic links as links, refusing a link that
  is absolute, dangling, or, resolved on the source side through every link
  on its way, leaves the framework, such as `R -> .` with `esc -> R/../x`;
  a file with a setuid, setgid or sticky bit is refused; every check runs
  before anything is written) and `<dir>/bundle.json` (each file's size,
  mode as copied and SHA-256, the six Info.plist files included, each
  link's target, the architecture, the CEF distribution record). The browser, the helper and the framework's library
  must be 64-bit Mach-O files of one architecture (the framework may be
  universal), the framework's sandbox library must be there, and a browser
  with the fixture inspection seam is refused unless `--fixture`. Nothing is
  signed: the Rust linker's ad-hoc signatures only (Apple silicon requires
  one), the framework as CEF distributes it.

```bash
cd native-tests/cef-candidate
CEF_PATH=/absolute/owned/cef cargo build --locked --bin takosumi-cef-browser-process --bin takosumi-cef-browser-helper
cd ../..
node scripts/bundle-macos-cef.mjs --cef /absolute/owned/cef/154.0.34/cef_macos_aarch64 --bin "$CARGO_TARGET_DIR/debug" --out /absolute/owned/m1
python3 -B native-tests/cef-candidate/windows_smoke.py --browser /absolute/owned/m1/takosumi-cef-browser-process.app/Contents/MacOS/takosumi-cef-browser-process --work /absolute/owned/cases --expect-sandbox
```

**Smoke** (`windows_smoke.py`, now also macOS). It runs the executable inside
the bundle directly, with the App's switches and `--use-mock-keychain` (so
Chromium never asks the login keychain, whose prompt would block an
unattended run; the App's adapter on macOS still has to decide how cookies
are encrypted), `TMPDIR` in its case and `MallocNanoZone=0` (CEF asks for
the nano allocator off; the bundle's `LSEnvironment` applies only to a
LaunchServices launch, so a direct launch, the smoke's and in M2 the App
adapter's, must set it itself; the `a8e4b08` logs held no nano-zone
warning before), in its own session and process group.
It requires the boot `opened` reply (cache path, native pid), `closed` and
`stopped` after the shutdown, exit code 0, and no process left: none in the
process group and none of the browser's descendants, recorded by parent pid
from `ps` before the shutdown with their command lines, still there as the
same process (same id and command line) within 10 s (macOS has no Job
Objects, and a descendant that outlives the browser is reparented to
launchd, so the tree alone would lose it). A failed run's cleanup kills
only those, so an id the system reused is never killed. With `--expect-sandbox` it
requires at least one renderer (`--type=renderer`) among the descendants,
each in the seatbelt sandbox (`sandbox_check(pid, NULL, 0)` from libSystem
answers 1) and without `--no-sandbox`, and no `--no-sandbox` for the
browser; the summary records it as `sandbox`. `--expect-load` (a
`fixture-inspection` build) requires the main-frame load marker; `--reuse`
requires the same browser and window id back and no raise refusal on the
browser's standard error. Whether the reused window became the active one
is not checked on macOS: activation there is cooperative (macOS 14 and
later may decline it), so the raise is not verified beyond the calls.

**CI** (`cef-macos`, `macos-latest` arm64, 75 min). It reads the locked
cef-dll-sys version, restores the CEF distribution and compiler caches
(keys of their own), installs Ninja when the image lacks it, builds
`takosumi-cef-browser-process` and `takosumi-cef-browser-helper`
(`--locked`, `CEF_PATH=$RUNNER_TEMP/cef`), lists the distribution's
framework in the log, bundles them (`m1-product`), runs the smoke with
`--expect-sandbox`, then builds the `fixture-inspection` browser, bundles it
with `--fixture` (`m1-fixture`) and runs the smoke with
`--expect-load --reuse --expect-sandbox`. The artifact `cef-macos-smoke`
keeps the summaries, their top-level logs and both `bundle.json`; no
binary. ci.yml now has seven jobs (nine runs with the `desktop` matrix).

**Remains after M1:** the guard on macOS (M2: no pidfds or Job Objects; a
process group with kqueue's process events and a descendant check), the
App's adapter and the actual App fixture on macOS, packaging (the browser
bundle inside the App's `.app`, a `.dmg`), Developer ID signing of every
Mach-O file (the framework, its libraries, the helpers; hardened runtime and
the entitlements Chromium's helpers need, such as JIT for the renderer) and
notarization (release inputs), the login keychain decision, Intel Macs
(`x86_64-apple-darwin`; the bundler accepts it, CI does not run it), older
macOS versions and real devices.

## macOS guard and adapter (M2)

Status: implemented on 2026-11-07. The first macOS runs (CI at `0b281ae`,
run 42389517653, and `3874b52`, run 42391603085, 9/9) passed all twelve
guard self-tests then and the adapter tests through the real Process API
(open, reuse, shutdown with a normal receipt, the App's death); the first
run's one failure was the unit test's expected reason (tauri's mock App has
no resource directory on macOS), fixed in `3874b52`. A review then asked for
the freeze before the kill, `NOTE_FORK`, the startup signal order, a
stricter group listing, the system's `TMPDIR`, a stricter death case and
the compile checks below; CI at `5663667` passed 9/9 with them: thirteen
guard self-tests, the six adapter tests, every freeze line (41) showing
all its stopped processes stopped, and the death case ending the browser
with `rootExit` -9 (`SIGKILL`). A last review asked that nothing between
the first `SIGSTOP` and the kill can panic or leave a process stopped (a
drop guard, a non-panicking log line), that unreadable stopped processes
are still killed, the kill goes leaves first, forks reported after the
freeze are looked at (or make the run incomplete), a reused parent pid
pulls in no children, base's macOS socket template, and two test
hardenings. That revision passed in `bc9fd69`'s
[CI run 42400222011](https://github.com/tako0614/takosumi-mobile/actions/runs/42400222011):
9/9 jobs, fourteen guard self-tests and six adapter tests. The App-death
case reported `rootExit: -9`, a complete receipt, no remaining process and
no cleanup errors. These are runner/component results, not an actual macOS
App UI or real-device qualification. This Linux host type-checks and lints everything for `aarch64-apple-darwin`
(`cargo clippy`): the guard and its self-tests, the browser and the helper
(`cef/dox`), the app-screen plugin with `desktop-process-experiment` (with
and without `fixture-overrides`) and the App with `desktop-cef`
(libsodium's build script pointed at an empty `SODIUM_LIB_DIR`, so it
builds nothing); nothing is linked or run here. The second macOS step
mirrors W2: `takosumi-browser-guard` for macOS, and the plugin's
`desktop-process-experiment` (and so the App's `desktop-cef`) compiled for
macOS, where it starts the guard and the bundled browser through the same
`Process` code as on Linux and Windows. M2 did not cover the App UI;
the M3 fixture below adds that candidate scope.

The later `6eacebe` run (42486555333) kept the same macOS implementation but
failed ordinary adapter shutdown: browser exit 0, complete receipt, no
remaining process, `forcedCleanup: 1`. Its freeze saw one stopped process;
the original log does not identify it. The App correctly retained the
profile. GPU error lines alone do not identify the forced helper; the cause
remains unknown. The guard now logs up to 32 counted PID/start identities
after the kill, with an optional freeze snapshot (parent, group, bounded
kernel process name and whether it showed stopped). A null snapshot means
no retained freeze observation, not proof of a late spawn. Names may be
truncated and are not CEF roles. No arguments or environment are logged.
The shutdown bounds and `forcedCleanup == 0` qualification are unchanged.
The diagnostic head `574cec4` passed all nine PR jobs (42488149912), including
14 guard and 6 adapter cases. The same tree merged as `0d480b0`; its automatic
main run (42489988073) failed ordinary shutdown again. The counted helper
PID 6082 was born after the initial tree snapshot, and later had parent 1,
its own process group 6082 and a stopped freeze snapshot. GPU errors named
PID 6065 instead. The truncated kernel name does not identify helper 6082's
role, and its start during page activity/reopen versus shutdown is still
unknown. This is a surviving helper, not evidence that a forced kill should
be accepted as normal.

The next diagnostic records helper entry (after sandbox initialization),
the return from CEF, and the point before helper exit, plus the browser's shutdown request and
CEF shutdown begin/return. Each best-effort stderr line contains its own
PID/start/parent/group, wall-clock microseconds, exit code when available,
and only closed enums for recognized process/utility roles. Unknown switch
values become `other`; no arbitrary argument, path or environment is logged.
Failure to read its own start time records null, not a fabricated identity.
This adds no work inside the guard's stop-to-kill interval and changes no
shutdown bound or receipt acceptance. Abnormal closes have a distinct request
label; the before-exit record is not proof of OS exit, and a missing best-effort
line does not prove that a helper was absent. The main RED remains unresolved.
The existing-browser reuse path also records focus begin/return, window-raise
return and reply return with the same closed metadata. These stages distinguish
a resume that stops inside focus, AppKit window raising or reply construction;
they do not prove foreground activation or successful delivery of a reply.
Missing lines can still be diagnostic loss. No operation or timeout is changed.
The App adapter's opted-in Mac inspection also records dispatch/blocking/lock,
send completion, reader enqueue, matching receive, native/cache confirmation
and IPC return boundaries. Reader quit/permission callback begin/return stages
distinguish an in-flight event callback from an unreceived open reply. Only
stage names, App/guard PIDs, assigned request IDs, success flags and wall-clock
microseconds are recorded; an unassigned guard or request has ID0. No key, profile URL,
reply body, path, environment or page data enters these lines. The record is
formatted before one best-effort stderr write. These are diagnostic candidates:
none proves delivery beyond its own boundary or changes ownership/timeout rules.
The first diagnostic run (`d32f18a`, 42491760278) passed its macOS component
job, but concurrent helpers interleaved the JSON formatter's separate writes.
Records are now formatted fully before one best-effort write. A failed or short
write still loses evidence; it does not delay or change helper shutdown.

**Guard** (`src/guard_macos.rs`, chosen by `src/guard_entry.rs`; the Linux
and Windows guards are unchanged). Same command line
(`<absolute new receipt> <program> [args...]`), receipt fields and exit
codes. macOS has neither Linux's child subreaper and pidfds nor Job
Objects, and kqueue's `NOTE_TRACK` (following forks) is gone, so the guard
owns the browser's processes by three means:

- **Its process group.** The App starts the guard as the leader of a new
  process group (`process_group(0)`); a guard that is not its group's
  leader refuses to start (exit 2). The browser and everything it starts
  inherit the group unless they leave it; every member but the guard is
  the browser's (`proc_listpgrppids`). libproc answers a failed listing
  with an empty one, so a group listing that does not hold the guard
  itself is a failure (errno is also checked on an empty answer), and a
  member whose state cannot be read counts as still there.
- **Descendants recorded while they run.** The guard lists the group and
  the children of every process it already tracks (`proc_listchildpids`,
  only while that pid still has the recorded start time, so a reused pid
  pulls in no one else's children) on every kqueue event, at least every
  250 ms, and at once when a tracked process forks (`EVFILT_PROC`
  `NOTE_FORK` on each one), and keeps each process it has seen by pid and
  start time, so a descendant that left the group (`setsid`, `setpgid`) is
  still found under its parent, and one later orphaned to launchd is still
  tracked.
- **PID reuse.** The start time comes from `proc_pidinfo`
  (`PROC_PIDTBSDINFO`). A child that started before the parent it was
  found under is not that parent's (the pid was reused) and is not
  recorded; a tracked pid whose start time changed is a different process
  and is never signalled; a zombie counts as ended.

It refuses (exit 2, nothing started, no receipt) a relative or existing
receipt path and a guard outside its own group; it reserves the receipt
(`O_EXCL`, mode 0600, never through a link), marks every descriptor it
inherited beyond the standard three close-on-exec, and starts the browser
with the guard's stdin, stdout and stderr (the App's control pipe, reply
pipe and log). `SIGTERM` and `SIGINT` are blocked from the guard's start
(the App also starts it with both blocked) until kqueue watches them
(`EVFILT_SIGNAL` records every one sent from then on, blocked or ignored);
one already pending then is a stop request before the browser starts.
Then the guard ignores both and unblocks them; the browser gets them back
at their defaults with an empty signal mask. It waits on one kqueue,
without polling or a lifetime cap, for the browser's exit (`NOTE_EXIT`),
a stop request (either signal, the App's stop as on Linux) or the App's
death (`NOTE_EXIT` on the guard's parent: macOS has no parent-death
signal; a parent already gone when the guard starts is a stop before the
browser starts, and a changed parent at any later look is the App's death
too, should its exit event be missed).

The drain then always runs: up to 0.5 s for the browser and
every tracked process to end by themselves. After observing a clean root
exit with no supervision failure, the natural window is 1.5 s instead.
Chromium's code-sign-clone cleanup polls for its parent's exit once a second
before deleting the temporary clone; the longer normal-exit window allows
one poll plus 0.5 s scheduling/deletion margin. Stops/App death observed before
drain, recorded observation failures and abnormal exits retain 0.5 s. The phase
is fixed at drain entry; later events never restart or extend it.
No role or process is exempt, and the role diagnostic
does not select this deadline. Then the guard **freezes the
tree before it kills anything**: `SIGSTOP` to the unreaped browser
(always), to each tracked process still the same (start time) and to
every other running member of the group, each verified right before its
signal; it looks again for processes they started before they stopped and
stops those too, until a look taken after every stopped process showed
stopped (`SSTOP`) finds nothing new and no stopped process has reported a
fork since, for at most 0.5 s. A one-shot kill of a listing could leave a
child forked meanwhile (or the child of a process outside the group,
orphaned when its parent died first). A stopped process should not fork
or exit by itself, and its children stay listed under it; a fork or spawn
a thread was already making may still complete after the stop (as XNU's
code suggests; not verified), which is why a fork reported after a stop
means another look. Then `SIGKILL` to every stopped process, **leaves
first**: each level of the stopped tree gets up to 100 ms to be gone
before its parents are killed, so no stopped child is orphaned while it
still runs (a process group orphaned with stopped members gets `SIGHUP`
and `SIGCONT`). A stopped process keeps its pid, so one whose state
cannot be read is killed too, its error recorded. For the rest of a 2 s
window, counted from the freeze, the guard kills whatever it still finds
the same way every 20 ms; at its end every stopped process still there
(or unreadable) is killed again and makes the run incomplete, and so does
a fork that a stopped process reported after the freeze's last look
(`ForkAfterFreeze` in `cleanupErrors`). From the first `SIGSTOP` to that
point a drop guard kills every stopped process should the drain return
or unwind early, and nothing in that section may panic: its arithmetic
saturates, and the one diagnostic line, written after the kill to
standard error (the profile's `browser.log`),
`takosumi-browser-guard: freeze {"stopped":N,"shownStopped":S,"freezeMs":T}`,
ignores a failed write (`eprintln!` would panic on a full disk or a
closed log, leaving the tree stopped). The guard's other stderr lines
ignore failures the same way. `forcedCleanup` counts, for the browser and
every other process alike, the processes the guard's `SIGKILL` reached
while they ran (verified, not a zombie): all but certainly ended by that
kill for a process the freeze saw stopped (it could only have ended
meanwhile by another's signal); for one it did not see stopped (the
freeze ran out, or it was found only after the kill), one that ended by
itself at that moment is counted too, since macOS gives the guard no exit
status for a process that is not its child: the count errs only on the
safe side (the App keeps the profile). A failed signal is a
`cleanupErrors` entry and makes the receipt abnormal. `remaining` lists
the pids still in the group or tracked; `observed` counts every process
seen, by pid and start time (the browser included, once).

**What completion means on macOS.** The receipt keeps the fields of the
Linux and Windows guards, but `kernelNoChildren` and `complete` mean "no
survivor among the processes the guard observed": after the browser was
reaped, the group lists only the guard and no tracked process is alive.
That is not a kernel proof, as the child subreaper's `waitid` answering
`ECHILD` is on Linux or the job's accounting is on Windows. Outside it: a
process the browser started through a system service (launchd, XPC), which
is not its descendant (as on Linux one started by a session manager is
not), and a double fork faster than an observation: a process that leaves
the group and whose parent ends before the guard's next look (the
`NOTE_FORK` wake, or the next tick) is orphaned to launchd unseen. CEF's
renderers stay in the group in the M1 smoke. Its code-sign-clone cleanup
instead starts its own group, and the guard tracks its observed browser
parent and birth identity. The App's checks
are unchanged (`require_normal_guard` states the same meaning).

**Self-tests** (`src/guard_macos_tests.rs`, `harness = false`, like
Windows'). The test binary re-executes itself as the guard, as stand-in
browsers and as a stand-in App, and runs fifteen cases: a normal exit; a
non-zero exit; the browser keeps the pipes and gets the default stop
signals with an empty mask; a lingering descendant is killed and counted;
a descendant that left the group (`setsid`) is still killed; one that ends
by itself is not forced; a self-grouped cleanup child that acknowledges a
live browser parent, polls again after one second and deletes its owned clone
fixture must finish with zero forced cleanup and an exact completion marker;
the handshake models the upstream poll, but an unusually descheduled browser
can shift its exit relative to that tick, so this case alone is not a
deterministic reproduction of every old 0.5 s failure;
a tree forking as the kill begins (a `setsid`
breeder forking a child every 5 ms for 2 s, which reports on a pipe once
its first child exists before the browser exits) is frozen, then killed,
with no child left in the breeder's group, at least two forced and every
observed process but the browser counted forced once; a guard whose
standard error cannot be written (a pipe with its reader closed) still
kills the stopped browser and writes a complete receipt, exiting 1 rather
than panicking; a stop
request drains a running browser; the App's death (`SIGKILL` of the
stand-in App) ends the guard and a browser that never reads its input
(`rootExit` `SIGKILL`, one forced); refusals create no receipt and start
no browser (a stand-in that would leave a mark); a browser that cannot
start leaves an incomplete receipt; the pid-reuse, zombie, stopped and
group-listing rules; the wait status as `rootExit`. The cases that start
processes require every one of them gone. Selecting no case fails.

**Adapter** (`src/desktop/process_adapter.rs`, the macOS pieces in
`process_macos.rs`; the POSIX code is shared with Linux):

- The guard starts from the App's spawner thread in its own process group,
  with the control pipe, the reply pipe and `browser.log`, and with its
  stop signals blocked; a fork whose parent is no longer this App refuses
  to run (`getppid` in `pre_exec`; Linux additionally arms
  `PR_SET_PDEATHSIG`). The App's death ends the guard through the guard's
  kqueue on its parent; an ordinary stop is `SIGTERM` to the guard, as on
  Linux.
- `confirm_opened` accepts the `opened` reply only when its native process
  is the guard's child, started no earlier than the guard, running exactly
  the executable the App located inside the bundle's `Contents/MacOS`
  (`proc_pidpath`), and when the reported cache path is exactly the
  requested one.
- The runtime root is `<app cache>/cef`, as on Windows (no XDG runtime
  directory), private to the user; the receipts live there. Chromium on
  macOS takes its temporary directory from `NSTemporaryDirectory()`, which
  prefers the system's per-user one (`confstr(_CS_DARWIN_USER_TEMP_DIR)`;
  only `MAC_CHROMIUM_TMPDIR` overrides it), so the `TMPDIR` the browser is
  given (that same directory, read with `confstr` rather than from the
  App's environment) is informational. Its singleton socket there,
  `<dir>/.<base bundle id>.XXXXXX/SingletonSocket` (base's macOS
  temporary-file template; the check takes the longer of the bundle's
  `com.takosumi.cef-browser` and Chromium's `org.chromium.Chromium`), must
  fit 104 bytes, which a path under the App's cache directory would not. A
  directory too long for it makes the browser unavailable with that reason
  (checked when the App locates the browser, and at each start).
- `HOME` stays the person's own. Every Chromium profile file is under the
  profile's cache path; what profiles share is the browser bundle's own
  per-user macOS state (its defaults domain, saved window state, system
  caches). A per-profile `HOME`, as on Linux, would not separate that:
  Cocoa looks up `~/Library` through the user's account rather than
  `HOME`, and overriding it is not known to be safe for CEF. Known limit,
  not implemented; M3 can list what the browser writes under `~/Library`.
- `clear_profile` refuses while Chromium's `SingletonLock` (a link to
  `<host>-<pid>`) names a running process on this host; a lock of another
  host (whatever its pid) or one that cannot be read counts as held, and a
  lock left by a holder killed with `SIGKILL` is stale.
- The environment is cleared and only `PATH`, `USER`, `LOGNAME`, `LANG`,
  `LC_ALL`, `LC_CTYPE` and `HOME` pass, with `TMPDIR` (above) and
  `MallocNanoZone=0` added (the bundle's `LSEnvironment` applies only to a
  LaunchServices launch); the browser gets `--use-mock-keychain`, so
  Chromium never prompts for the login keychain and encrypts cookies with a
  fixed key (whether the App should use the login keychain is still open).
- External links open through `/usr/bin/open <url>` in its own process
  group, reaped by the thread that starts it, within the same budget of
  eight openers. A `fixture-overrides` build only appends the link to the
  recording log; the unit tests only record it and never start an opener.
- The packaged names are
  `takosumi-cef-browser-process.app/Contents/MacOS/takosumi-cef-browser-process`
  and `takosumi-browser-guard`; where the App's bundle holds them is M3's
  packaging.

**Adapter tests** (`macos_tests` in `process_adapter.rs`). Four need
nothing built: an unresolvable browser leaves the App running with its
reason (on macOS the mock App's missing resource directory) and without
the clear capability; a held `SingletonLock` refuses the clear and keeps
the data, while a foreign or garbled lock counts as held and a stale one
as free; another host's lock refuses even with a pid free here, and a
holder killed with `SIGKILL` leaves a stale lock the clear then removes;
the identity checks read this process (parent, start time,
`proc_pidpath`) and the system's temporary directory and socket bound.
Two are ignored unless asked for and need `TAKOSUMI_MACOS_GUARD` (the built
guard) and `TAKOSUMI_MACOS_BROWSER` (the bundled product browser's
executable; `TAKOSUMI_MACOS_WORK` places the case directories): one opens a
profile through `Process::spawn` and the creation reply check (a
cold-start bound of 120 s), restates the identity checks against the
kernel, requires a CEF helper below the browser and the profile's
`SingletonLock` held, reopens the screen and requires the same browser,
window, pid and cache back, shuts it down through the adapter and requires
a normal guard receipt and every process of the tree gone; the other
re-executes the test binary as a stand-in App (the seventh, ignored test,
which only that case runs) that opens a profile and hands a copy of the
browser's input to a holder process, so the App's death cannot end the
browser through its input, then kills that App with `SIGKILL` and requires
the guard, the browser and the whole tree gone within 10 s, and the receipt
complete with `kernelNoChildren`, nothing remaining, `rootExit` `SIGKILL`
and at least one forced process: the guard's watch on the App ended it.

**CI.** `cef-macos`: the product build also builds
`takosumi-browser-guard`. After the product smoke, `cargo test --locked
--bin takosumi-browser-guard` must report `15 passed; 0 failed`; the
plugin's lock is pruned from the App's (its own step, as in the Windows
job); `cargo test ... --features desktop-process-experiment --lib --
macos_tests --include-ignored --skip app_stand_in_for_the_death_case
--test-threads 1` runs with the built guard and `m1-product`'s browser and
must pass each of the six tests by name and report `6 passed; 0 failed;
0 ignored`; and `cargo check` compiles the plugin's macOS product build
(`desktop-process-experiment`: `open_externally` through `/usr/bin/open`)
and fixture build (`fixture-overrides`: the recording opener). The
`desktop` job's macOS run compiles the App with `desktop-cef`, as its
Windows run does. The artifact `cef-macos-smoke` adds both test logs and
each case's `browser.log`, `cef-native.log` and kept guard receipt: text
only. Still seven jobs (nine runs).

**Remains after M2:** native qualification of the M3 actual App fixture below,
packaging (the browser bundle and the guard inside
the App's `.app`, a `.dmg`), Developer ID signing of every Mach-O file with
the hardened runtime and the entitlements Chromium's helpers need, and
notarization (release inputs), the login keychain decision, the shared
per-user macOS state of the browser bundle (the `HOME` decision above),
Intel Macs, older macOS versions and real devices.

## macOS actual App fixture (M3)

Candidate source, not a complete native PASS. The explicit
`native-browser-process-fixture` feature now uses the same App flow on
Linux, Windows and macOS. The Mac driver uses Tauri's `eval_with_callback`
against the real WKWebView home, with no new dependency or product UI.
`native-tests/desktop/macos_browser_process.py` runs anonymous Excalidraw
and Photopea separately; it does not sign in or supply real account data.

Each case selects a random persistent WK store UUID, refuses macOS below
14, requires a fresh first home and restores its saved Work/Personal
connections on two later launches. The pinned tauri-runtime 2.11.3 config
conversion drops `data_store_identifier`, so the fixture disables automatic
creation of `main` and explicitly calls the window builder's
`data_store_identifier` before starting the driver. A configuration marker
alone is not runtime proof: both distinct cases and their restarts must pass.

The observer requires two separately guarded CEF profiles, separate synthetic
cookie/localStorage values, same-document reuse through the home, sequential
quit cancellation and partial quit, restart persistence, clearing only Work,
refusing a clear while Personal runs, then clearing Personal after its normal
close. It checks PID/birth, parent and executable identity, the exact loopback
inspection listener and renderer sandbox. Every profile and the outer App
guard must exit normally with zero forced cleanup and no remaining tracked
process. A completed page or closed CDP socket never substitutes for a receipt.

The existing `cef-macos` job builds the actual App after the M1/M2 steps and
runs both sites once, each in three launches. Its text artifact also retains
App/observer summaries, home-store requests, guard receipts and bounded tails
of this case's browser logs. It includes no browser database or binary. The
three portable rejection tests exercise listener/process ownership and abnormal
receipts; Linux cross-Clippy with C/library stubs checks Rust only, not linking
or native behavior.

Mac lifecycle records also mark the existing control-packet read, UI-task post
return value and UI-task entry, without parsing or logging packet contents.
Run43297320017 cancelled quit successfully, then Work's reuse-focus began
25.176 seconds after the App's successful send; its reply arrived after the
App had already failed its wait. The new boundaries distinguish reader/post
delay from UI-task delivery, with the existing command/focus markers covering
dispatch and focus. Post return and task entry run on different threads and
may be recorded in either order; FIFO packet order and positive command
records must be correlated. A missing best-effort line proves no stop point.
The synchronous diagnostic write retains the existing control-lock scope
and can affect timing. No protocol, scheduling or timeout rule changes, and
native collection of these boundaries remains unqualified.

M3 uses CDP `Page.close` on relaunch. OS window switching, keyboard focus,
real login/popup return, native files/downloads, site permissions, notifications,
OS-managed WKWebView services outside the App's guarded tree, macOS packaging,
signing and updates remain separate. The earlier ordinary-shutdown RED is
retained; no grace period or receipt acceptance is relaxed for this fixture.

The first M3 native run (42493648973 at `3ad4ca6`; CI's synthetic merge
`b159ded` has the same tree) built and ran the actual Mac App. Both sites
passed separate-profile synthetic cookie/storage and same-document reuse.
Excalidraw's beforeunload opened and closed with `result:false`, then the
browser threw `NSInvalidArgumentException`: `TakosumiCefApplication` lacked
`cancelTerminate:`. Its guard recorded signal 5, zero forced cleanup. The
application now implements that selector and forwards to the delegate's
`stopTryingToTerminateApplication` when present, matching
[the pinned Chromium 154.0.8037.98 cancel route](https://github.com/chromium/chromium/blob/154.0.8037.98/chrome/browser/chrome_browser_application_mac.mm)
without invoking AppKit termination. The real cancel/reuse/subsequent
normal-close flow remains the regression gate, not just selector presence.

Photopea instead timed out waiting for beforeunload opening; no dialog was
recorded and no prior crash was established. Its outer forced cleanup was
the runner's response to failure, not an ordinary exit. The Mac observer now
keeps dialog events, target/receipt state, setup activation/visibility and
checkpoint times before that cleanup. The original wait bounds and required
open/cancel events remain unchanged.

The next native run (42495346362 at `0c4013c`; synthetic merge `1ce6eab`
has the same tree) passed all three Excalidraw launches: cancel/reuse,
sequential quit, restart with persistent separated state and one-profile
clear. All nine App/profile receipts had exit 0 and zero forced cleanup.
Photopea passed its first launch, including both cancellations and normal
exit, and verified the separated state after restart. Closing Personal on
that restart entered `CefShutdown`, then printed `Teardown watchdog expired`
and exited 2; the guard had to end the network-service helper. Reset was
not reached. The watchdog's cause remains unknown; a surviving helper is
not by itself proof of what blocked shutdown. This differs from both the
earlier missing dialog and the old M2 exit-0/forced-1 failure.

The same run's M2 adapter failed during its first creation confirmation,
before reuse or shutdown. Its native window existed, but the exact failed
reply predicate was not logged. These failures keep Mac native acceptance
open despite the completed Excalidraw flow.

The next diagnostic records only admission predicate booleans on a rejected
Mac creation reply, including the exact libproc reads used for admission.
It neither retries nor relaxes identity/path checks. In the Mac fixture,
an owned browser still inside `CefShutdown` after two seconds is sampled
once for one second at 100 ms intervals, before Chromium's ten-second
[teardown watchdog](https://github.com/chromium/chromium/blob/154.0.8037.98/chrome/browser/shutdown_watchdog_mac.cc).
PID/birth, guard ancestry and both executable paths are checked before and
after; changed identity discards the output. The sample subprocess has a
five-second limit and passes `-mayDie`, so symbol locations are grabbed
before the sampled browser can be watchdog-killed mid-run; the recorded tool
support was kept for exactly this after a bare run timed out without a
stack, and the bound stays under the watchdog while letting symbol grab,
the sample and the report write finish on a CEF-sized binary, which three
seconds did not. Its retained stack text is capped at 4 MiB and stderr at
8 KiB. A timeout retains any partial stack and stderr only after the same
identity readback, and remains a failed diagnostic, never a native pass.
The subprocess is reaped and the watcher joined before fixture cleanup.
Filesystem/libproc reads have no hard IO deadline. A failed
watcher-thread start still runs the close body; missing samples never change
the receipt verdict. Sampling
can perturb scheduling: a sampled pass is diagnostic, not an unsampled
qualification. No heap, environment, page contents or binary crash dump is
requested. Existing first-launch/restart/reset and zero-forced-cleanup
requirements remain unchanged.

The CI also records the installed `/usr/bin/sample` usage (with no process
argument) and its installed manual, each capped at 16 KiB, in
`macos-sample-capabilities.json`. This read only identifies available diagnostic
options: it attaches to no process and does not select a new flag or extend the
sampling/shutdown deadlines. It is needed when symbol processing times out
without leaving a stack, so the next diagnostic can use observed tool support.

At `6a1f7ff` (run 42497100922), Photopea completed all three launches with
normal receipts and no stack sampling. Excalidraw instead stopped at the
partial quit: Work exited 0 but its guard forced one leftover process, so
the App correctly kept Personal open. The leftover was born 10.3 ms before
Work's `CefShutdown` returned; by the freeze it had parent 1 and its own
process group, with no helper-entry record identifying its role. This is
not evidence of a missing Personal beforeunload. The full run was 8/9,
with only the Mac job failing; the earlier creation refusal did not recur.

The guard now retains the first parent/group already observed for each
PID/birth and includes it in forced-process diagnostics after kill/disarm.
There are no new OS queries, argument captures, startup hooks or waits.
`trackedParentStart` is the parent's stored observation, not a fresh
simultaneous identity proof; a missing value stays unknown. The escaped
descendant test checks that its first parent survives later orphaning,
allowing the first group to be either side of `setsid()`. The fixture's
existing stack watcher also covers Work's partial close and keeps both
partial/final records. These changes do not resolve the residual process,
the earlier Photopea watchdog or the creation refusal by themselves.

At `48de24f` (run 42498855602), Excalidraw completed all three launches
without sampling, and Photopea completed its first and restart launches
with normal receipts. Photopea's reset removed only Work's profile, then
failed to reopen it. The M2 App-death test also failed during creation.
Both rejections had matching parent/birth, key, profile, cache and positive
native IDs; the executable path read by `proc_pidpath` did not exactly
match the expected canonical path. The earlier watchdog and late helper
did not recur; the reset's outer forced cleanup is failure cleanup, not a
normal-shutdown result.

The rejection diagnostic at `a35a531` identified Chromium's code-sign clone:
its main executable was a hard link of the selected browser (`sameFile=true`,
stable PID/birth, different canonical path). The adapter now holds that
executable open before spawn and compares regular-file device/inode, with
process identity and image-path rechecks. Copies and replaced files fail;
the observer and shutdown sampler also hold their expected executable files.
This is not bundle/signature validation or an atomic running-vnode check
against concurrent same-user filesystem mutation.

At `fe1b913` (run 42502662408), all nine Mac adapter/image tests passed, and
both anonymous sites completed all three App launches with nine normal
receipts each. Photopea had no stack sampling. Excalidraw's first launch
attempted one sample, then discarded it after the process identity changed;
its timing may have been perturbed. The two Windows fixture jobs instead
failed a stale CI-contract test that counted only the six adapter tests,
excluding the three new image tests; that test now checks both exact sets.
The earlier intermittent late helper and teardown watchdog remain unresolved.

`TAKOSUMI_MACOS_GUARD_ROLE_DIAGNOSTICS=1` opts native diagnostics into one
closed-role read for a tracked orphan in its own process group after natural
drain expires, before any SIGSTOP. Default runs do not query arguments.
The guard reads at most 64 KiB using KERN_PROCARGS2, checks the required size
before/after, parses exactly the counted arguments (including exact 64-bit
path padding and empty-argv boundaries), and discards raw bytes. It records
only `none`, `other`, `code-sign-clone-cleanup` or null plus PID/birth, identity
stability and elapsed time, after kill/disarm. No path, argv or environment
value is logged. This does not grant any role an exception: the native
escaped-helper test claims the cleanup type and must still be killed/count 1.

Run 42504487318 at `ae375e4` passed the four native parser and fourteen guard
tests, including the escaped helper's actual kernel role. Excalidraw passed all
three App launches without stack sampling. Photopea's Work browser exited 0 but
its guard forced one late own-group helper; the App retained the profile and
did not advance to Personal's beforeunload. This is still a failed ordinary
shutdown. The App cleared the diagnostic flag when launching profile guards,
so that helper's role was not observed. The forwarding repair passes only
literal `1`, only in an inspecting fixture build; normal product builds never
read or forward it. It changes no cleanup/acceptance rule.

The forced cleanup deadline starts before this optional read; the read itself
never extends the selected natural or forced window. A sysctl has no hard latency bound and
may return stale data across same-PID exec, so the result is observational.
Opt-in runs are timing-perturbed evidence, not unsampled shutdown qualification.
The pinned [XNU layout](https://github.com/apple-oss-distributions/xnu/blob/xnu-10063.121.3/bsd/kern/kern_exec.c#L5467-L5481)
and [sysctl behavior](https://github.com/apple-oss-distributions/xnu/blob/xnu-10063.121.3/bsd/kern/kern_sysctl.c#L1335-L1592)
explain the strict padding, size checks and exec-race limitation.

Run 42506394284 at `06c3b94` then observed the late Excalidraw restart helper's
actual `code-sign-clone-cleanup` role with stable PID/birth and the recorded
browser parent. It was forced after the old 0.5 s natural window. Photopea
completed all three launches. The [pinned Chromium cleanup loop](https://chromium.googlesource.com/chromium/src/+/154.0.8037.98/chrome/browser/mac/code_sign_clone_manager.mm#753)
sleeps for one second after seeing its parent alive, then removes the clone
only after reparenting. The normal-root-exit window above now accommodates
that behavior; its native regression also requires the owned clone fixture
to be removed. This repair still needs native execution and does not resolve
the separate historical `CefShutdown` watchdog failure by itself.

Run 42607719040 at `cc4f836` reproduced the historical watchdog under
Photopea: after Personal's `system-quit-request`, the serialized quit sent
Shutdown to Work, Work closed its windows and logged `cef-shutdown-begin`,
then hung inside `CefShutdown` until Chromium's ten-second teardown
watchdog killed it (rootExit=2). Personal therefore never received
`shutdown-request` and its beforeunload never opened - a failed ordinary
shutdown, not evidence of a missing handler. The hanging profile's
`cef-native.log` recorded `Failed to post task from
FlushAndNotifyInBackground to client_task_runner_` (a sqlite cookie-store
flush post failing during teardown); Photopea writes cookies continuously,
so a shutdown-ordering race is plausible but unproven and still needs a
sampled stack. The run added the `-mayDie` sample flag so symbol locations
are grabbed before the watchdog can kill the sampled browser.

Run 42613143371 at `02ef39f` hung at the same CefShutdown stage on the
second of three Photopea launches; the `-mayDie` symbol grab still exceeded
the then-three-second subprocess bound, so `1a035be` raised it to five
seconds, keeping detect (~+2 s) plus sample inside the ten-second kill
window. The iOS job flaked on simulator boot, and native-plugins recovered
its picker test, marking that failure intermittent.

Run 42621435741 at `ceed2fe` then passed all nine Mac adapter/image tests
and all three Photopea launches through all six phases with no stack
sampling, so the five-second bound remains unexercised and the hang's root
cause unresolved. The same run's Android job still failed at the Open tap
stage (filePrompts=0); its retained screenshot proved the first paint gate
accepted the site's own uniform Loading backdrop, corrected by 03b33c3.

## macOS packaging (M4)

The embedded product-browser sandbox smoke passed. The packaged fixture's
anonymous Excalidraw and Photopea home/profile flows each passed all three
launches with normal inner/outer receipts in
[CI43202312305](https://github.com/tako0614/takosumi-mobile/actions/runs/43202312305).
This is local debug bundle evidence. Intermittent unbundled Photopea helper
retirement failures remain unresolved; these passes do not establish their
repair or qualify signing/distribution. On a prepared Mac with at least
8 GiB free, use the same CEF SDK and target as the M1 build:

```bash
CEF_PATH=/absolute/owned/sdk CARGO_TARGET_DIR=/absolute/owned/target cargo build --locked --manifest-path native-tests/cef-candidate/Cargo.toml --bin takosumi-cef-browser-process --bin takosumi-cef-browser-helper --bin takosumi-browser-guard
node scripts/bundle-macos-cef.mjs --with-guard --cef /absolute/owned/cef_macos_aarch64 --bin /absolute/owned/target/debug --out src-tauri/target/macos-cef/cef
CARGO_TARGET_DIR=/absolute/owned/target bunx tauri build --ci --debug --bundles app --features desktop-cef --config src-tauri/tauri.macos-cef.conf.json
```

The overlay maps this stage to `Takosumi.app/Contents/Resources/cef/`. The
existing adapter resolves the browser at
`cef/takosumi-cef-browser-process.app/Contents/MacOS/takosumi-cef-browser-process`
and the adjacent `cef/takosumi-browser-guard` without executable overrides.
The minimum macOS version is 14, as required by the shared home data stores.
The `.app` is a local debug artifact; Developer ID signing, notarization,
distribution and updates remain separate release work.

The browser sets CEF's `main_bundle_path` to its own enclosing
`Contents/MacOS` bundle. In this CEF version the empty default uses Chromium's
outermost `.app`, which would select the Tauri App and search for the browser's
framework and helpers there. CEF passes the explicit bundle path to its
helpers. See [CEF's bundle lookup](https://github.com/chromiumembedded/cef/blob/14c5a08/libcef/common/util_mac.mm#L116)
and [Chromium's outermost lookup](https://github.com/chromium/chromium/blob/154.0.8037.98/base/apple/foundation_util.mm#L154).
The SDK-free `mac_bundle.rs` controls cover both standalone and nested paths;
the embedded sandbox smoke and packaged App flow remain the native oracles.

`--with-guard` checks the guard's thin Mach-O architecture against the browser
before any copy, refuses a missing or linked guard, and never overwrites the
output. `bundle.json` records the guard's own bytes/mode/hash separately from
the nested browser app's entries. The existing framework link checks and
fixture-inspection rejection still apply. These checks do not qualify a
Mach-O's full linkability or native execution on a Linux test host.

The explicit fixture overlay `tauri.macos-cef-fixture.conf.json` uses
`src-tauri/target/macos-cef-fixture/cef`. A `--fixture` stage cannot be written
to the product's stage directory or its descendants, including through existing parent links.
The CI builds both `.app` artifacts, requires the same product guard in both
stages, and runs the product browser from inside its packaged resources with
the existing sandbox smoke. It then runs the packaged fixture's shared
home/profile flow separately on anonymous Excalidraw and Photopea:

```bash
python3 -B native-tests/desktop/macos_browser_process.py --packaged --binary <TakosumiFixture.app>/Contents/MacOS/<CFBundleExecutable> --work <owned evidence directory> --site excalidraw
python3 -B native-tests/desktop/macos_browser_process.py --packaged --binary <TakosumiFixture.app>/Contents/MacOS/<CFBundleExecutable> --work <owned evidence directory> --site photopea
```

This mode accepts no `--browser` or `--guard`. It removes inherited browser
executable/runtime overrides, lets the App choose its bundled resources and
per-user runtime directory, and attests the opened browser and guard against
those exact packaged executables. The same isolation, reuse, sequential quit,
restart, clear and normal inner/outer receipt assertions still apply. The
original unbundled M3 flows remain separate, so a packaging failure cannot
replace their results. Ordinary product home rendering, OS focus,
physical-device use and release signing remain unqualified.

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

## Windows same-App recovery fixture

The synthetic-site, same-running-App Windows recovery case passed all six
required cases in [CI43218039839](https://github.com/tako0614/takosumi-mobile/actions/runs/43218039839/job/131182189815).
The W3 job runs a separate fresh case with `--recover`, the same sandboxed
browser and the existing fixture TLS setup. It accepts no
restart/reset, real-site, installed-package or unsandboxed mode combination.

The actual home creates Work and Personal. The observer seeds both, normally
closes Work and verifies the saved cookie/storage after reopening it, then
terminates only Work's held native browser handle with a synthetic exit code.
App/guard/browser image paths, creation times and direct ancestry are checked
before that fault; the App and guard handles have no termination access. A
failed identity/liveness check refuses the fault, with no kill-by-PID fallback.

Ordinary open must only offer recovery; the actual confirmation button must
create the replacement. The observer requires the same UUID/cache directory
identities and saved connections, new native process generations, unchanged
Personal document/processes/data, and one incident preserving the original
abnormal receipt. Seed-normal, injected-abnormal and final-normal evidence
remain separate. Recovered Work, Personal and the outer App job must all end
with their original strict normal conditions; the injected failure cannot
satisfy a final cleanup check. Six named cases and a nonempty passed summary
are required.

The SDK-free controls exercise fault refusal, PID reuse and case wiring.
The held Win32 fault, actual Windows App recovery confirmation and final
normal cleanup ran in that native case. This does not qualify whole-App/OS-crash
recovery,
unsaved writes, actual authentication or installed-package recovery.
