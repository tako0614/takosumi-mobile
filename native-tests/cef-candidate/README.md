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
implies it. This section is about Linux; on Windows the feature compiles for
the unsandboxed candidate without a package (see
[Windows guard and adapter](#windows-guard-and-adapter)). It is never enabled
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

`takosumi-cef-browser-process` also builds for `x86_64-pc-windows-msvc`, as an
unsandboxed test candidate that must be asked for by name with the
cef-candidate feature `windows-unsandboxed-candidate`. The first Windows step
(W1, this section) ran the browser on its own, started by `windows_smoke.py`
or the CI job. The second (W2, [Windows guard and adapter](#windows-guard-and-adapter))
adds the Windows `takosumi-browser-guard` and lets the App's adapter start
both; nothing is staged or packaged.

```powershell
$env:CEF_PATH = "C:\absolute\owned\cef"   # the first build downloads the Windows distribution here
cargo build --locked --manifest-path native-tests/cef-candidate/Cargo.toml --features windows-unsandboxed-candidate --bin takosumi-cef-browser-process --bin takosumi-browser-guard
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

- **Sandbox.** Off on Windows in this candidate, whatever the switches. CEF's
  Windows sandbox needs cef-dll-sys's `sandbox` feature (`cef_sandbox`) and the
  bootstrap layout: the browser built as a DLL that CEF's `bootstrap.exe`,
  renamed to the browser, loads, with sandbox information passed to
  `execute_process` and `initialize`. This build links none of that, so it sets
  CEF's `no_sandbox` itself, and a Windows build without
  `windows-unsandboxed-candidate` is a compile error. The sandbox must be
  qualified before any Windows package (the TODO in `without_sandbox`).
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
  without the manifest. The CI job reads the `RT_MANIFEST` resource back from
  the executable with the Windows SDK's `mt.exe -inputresource:<exe>;#1` and
  requires exactly that `supportedOS` entry, the Common Controls 6 dependency
  and one `asInvoker` execution level.
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
- **Console.** The executable keeps the console subsystem. The App starts the
  guard, and the guard the browser, with `CREATE_NO_WINDOW` (W2), so neither
  opens a console window.

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

The CI job `cef-windows` on `windows-latest` builds the product browser with
`--locked --features windows-unsandboxed-candidate` (`CEF_PATH` is
`${{ runner.temp }}\cef`, cached by the locked cef-dll-sys version), checks
its embedded manifest and runs the harness. It then builds the
`fixture-inspection` browser with the same feature and runs it with
`--expect-load --reuse`.

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
through the same `Process` code as on Linux. macOS and other OSes still refuse
both with a single compile error. W2 had no App UI or fixture on Windows
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
`CREATE_NO_WINDOW` gives the browser a console without a window, and with it
a windowless console host (`conhost.exe`) child in the guard's job. CEF's
helper processes run the same console program and share that one hidden
console. Started detached (`DETACHED_PROCESS`, no console), each helper would
instead get a new console of its own, possibly a visible one, and a console
host each, so the browser keeps its hidden console. `observed` therefore also
counts that console host, and the drain waits for it like any other process
in the job. It then waits, without polling or a lifetime cap, for the browser's exit or a
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
reads may count twice). A listed process that was already gone when opened
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

**Parent death.** Windows has no `PR_SET_PDEATHSIG`. The adapter's spawner
thread owns one kill-on-close Job Object for the App and starts every guard
inside it (suspended, assigned, resumed). The guard is started
`DETACHED_PROCESS`: it needs no console, so it has no console host either;
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
(eleven cases in `src/guard_windows_tests.rs`; the self-test binary is its
own stand-in browser, and each case starts the guard detached inside a fresh
kill-on-close job as the App does): a normal end, a non-zero exit, the pipes
reaching the browser without the stop event, a lingering descendant
terminated with the job, a job that empties just as the natural budget ends,
a descendant that ends by itself after the process list was read but before
the termination (not counted as forced; self-test-only injections,
`TAKOSUMI_GUARD_FAULT_LATE_EMPTY` and `TAKOSUMI_GUARD_FAULT_TERMINATE_LATE`,
produce these two orders, which timing alone cannot produce on demand), a
stop request, the App's job ending guard and browser, a pending stop, the
refusals and a browser that cannot start. Cases that require the App's job
to be empty afterwards wait at most 3 s for processes whose exit is already
under way (the stand-ins that must be ended sleep 60 s) and name every
process still there with its image. `App adapter with the built
guard and browser (Process API)` runs the plugin's Windows tests with
`TAKOSUMI_WINDOWS_GUARD` and `TAKOSUMI_WINDOWS_BROWSER` pointing at the
product build: unit tests of the pipe writer (a write the reader does not
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
executable) and its console host (`conhost.exe`), and Chromium's `lockfile`
in the profile's cache root held, reopens the same key and requires the same
native browser and window, shuts down through `Process::shutdown` and
requires a normal receipt (complete, `rootExit` 0, no forced cleanup, the
job empty, and `observed` at least the browser plus the console hosts and
helpers held, so a helper outside the guard's job leaves it short), the
`lockfile` released and the runtime directory's removal.
`windows_adapter_ending_the_app_job_ends_guard_and_browser` starts a
profile, holds a running CEF helper and the console host open (a helper that
then ends by itself has still ended), records the guard's process tree,
drops the App's spawner (closing the App's job) and requires every process
held to end within 15 s, naming each survivor with its image, and no normal
receipt. A missing helper prints a fresh view of the browser's tree with
each process's parent and image and the expected program. The step fails
unless each named test passed and exactly thirteen ran. The `desktop` job also checks the App with `--features desktop-cef` on
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
- `observed` includes the browser's console host, so it is one higher than
  on Linux for the same browser.

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
cargo build --locked --manifest-path native-tests/cef-candidate/Cargo.toml --features fixture-inspection,windows-unsandboxed-candidate --bin takosumi-cef-browser-process --bin takosumi-browser-guard
bun run build
cargo build --locked --manifest-path src-tauri/Cargo.toml --features native-browser-process-fixture
pwsh -File native-tests/desktop/windows_fixture_tls.ps1 -Out C:\absolute\owned\w3-tls
python -B native-tests/desktop/windows_browser_process.py --binary <target>\debug\takosumi-mobile.exe --browser <target>\debug\takosumi-cef-browser-process.exe --guard <target>\debug\takosumi-browser-guard.exe --data "$env:TEMP\sumi-w3-local" --tls-cert C:\absolute\owned\w3-tls\cert.pem --tls-key C:\absolute\owned\w3-tls\key.pem --tls-ca C:\absolute\owned\w3-tls\ca.pem --restart --reset
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
W3, follow the app link at once, and must reach the site's readiness signal
within 60 s: the Excalidraw editor root (`.excalidraw`), or Photopea's
landing or editor text, the signals the Linux probes use
(`browser-process.mjs`). The site must load in a secure context with no App
bridge, in the profile's one window. Then, on the real pages:

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

The W3 invariants hold: the attested owner of each DevTools port, unchanged
source and executable hashes, no opener request. After the final quit
request the observer accepts any beforeunload a real site raises on its own,
and records it, so the quit can finish. The App's first checkpoint waits up
to 150 s and its watchdog 240 s in this mode
(`TAKOSUMI_BROWSER_PROCESS_REAL_SITE`), and the observer up to 230 s.
Anonymous use only: synthetic markers, nothing signs in anywhere real, no
credential is typed.

**Workflow.** `.github/workflows/windows-real-sites.yml` runs only on
`workflow_dispatch`, with the takosumi-mobile ref to test as its input. It
builds like `cef-windows-app` (restoring its caches without saving), trusts
the fixture CA, runs the Excalidraw and the Photopea cases in separate steps
(25 minutes each; the second runs even if the first fails), requires each
step's eight `<site>/<case>` names, exactly eight `PASS case` lines and a
passed `summary.json`, removes the CA whatever happened and keeps both cases
and the profiles' browser logs as the artifact `windows-real-sites-cases`.
GitHub lists a `workflow_dispatch` workflow for dispatch only once its file
is on the default branch.

**Not shown by W4:** Photopea's editor entry (the Linux run clicked "Start
using Photopea"; here its landing counts as ready), the long native alert,
downloads, popups, notifications and the native file chooser on the real
sites; a real login; and anything W3 does not show (see above).

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
