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

`takosumi-browser-guard` (`src/guard_main.rs`, compiled only for Linux through
`src/guard_entry.rs`) is the product supervisor that
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
implies it. It is Linux only, never enabled by default, and nothing here
publishes or signs a package. The first local packaging default (chosen by the
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

`takosumi-cef-browser-process` also builds for `x86_64-pc-windows-msvc`. This
first Windows step runs the browser on its own, started by `windows_smoke.py`
or the CI job; the App does not start it on Windows yet (`desktop-process-experiment`
and `desktop-cef` stay Linux only), and nothing is staged or packaged.

```powershell
$env:CEF_PATH = "C:\absolute\owned\cef"   # the first build downloads the Windows distribution here
cargo build --locked --manifest-path native-tests/cef-candidate/Cargo.toml --bin takosumi-cef-browser-process
python -B native-tests/cef-candidate/windows_smoke.py --browser <target>\debug\takosumi-cef-browser-process.exe --work C:\absolute\owned\cases
```

Build only that bin on Windows. The guard needs Linux pidfds, signalfd and the
child subreaper, and the standalone `takosumi-cef-candidate` is a Linux
experiment. Both refuse to compile on other OSes with a single error instead
of building an inert stub that a stage could pick up by mistake. A Windows
supervisor (for example a Job Object owning the browser's processes) is not
written. The build needs the MSVC toolchain, CMake and Ninja for cef-dll-sys's
`libcef_dll_wrapper`, as `windows-latest` provides.

The X11 raise (`x11_front`), the euid sandbox decision (`libc` is now a
Linux-only dependency) and the `$ORIGIN` run path are compiled only for Linux,
where the browser's behaviour and output are unchanged. On Windows:

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
- **Profile directory.** `TAKOSUMI_BROWSER_PROCESS_DIRECTORY` must be absolute
  and in its final form, as on Linux. On Windows the check drops the `\\?\`
  (or `\\?\UNC\`) prefix that `canonicalize` adds; short (8.3) names, links
  and letter case are still resolved, so the parent passes the long final
  spelling (the harness uses `os.path.realpath`). `boot.json`, `Downloads/`,
  `cef-profiles/<profile>/`, `cef-native.log` and the stdin/stdout protocol are
  unchanged. The Linux HOME/XDG/TMPDIR directories have no counterpart; the
  harness points `TEMP` and `TMP` into its case.
- **Window reuse.** Reopening a live screen restores a minimized window, shows
  it and calls `SetForegroundWindow` through minimal `extern "system"` user32
  declarations (the CEF wrapper already links user32). Windows hands the
  foreground only to a process allowed to take it, so an App should call
  `AllowSetForegroundWindow` with the browser's pid before sending the Open.
  Otherwise Windows flashes the taskbar button and the browser prints
  `Reused browser window was not raised: Windows refused the foreground change`
  to stderr, as it does for an X11 refusal.
- **Sandbox.** Off on Windows in this candidate, whatever the switches. CEF's
  Windows sandbox needs cef-dll-sys's `sandbox` feature (`cef_sandbox`) and the
  bootstrap layout: the browser built as a DLL that CEF's `bootstrap.exe`,
  renamed to the browser, loads, with sandbox information passed to
  `execute_process` and `initialize`. This build links none of that, so it sets
  CEF's `no_sandbox` itself. It must be qualified before any Windows package
  (the TODO in `without_sandbox`).
- **Console.** The executable keeps the console subsystem. An App starting it
  from a GUI process must pass `CREATE_NO_WINDOW` (or the browser must move to
  the Windows subsystem) to avoid a console window. Not handled yet.

**Smoke harness.** `windows_smoke.py` uses only the Python 3 standard library
(plus `tasklist`/`taskkill` on Windows) and runs on both OSes. It creates a new
case directory under `--work` (`tempfile.mkdtemp`), lays it out like the App's
adapter and writes the boot Open for `https://example.com/` with a new UUID
profile (key `web-home:<profile>:https://example.com`). It starts the browser
directly with `TAKOSUMI_BROWSER_PROCESS_DIRECTORY` and the App's switches (on
Linux also private HOME/XDG/TMPDIR, `LD_LIBRARY_PATH` and `--no-sandbox` as
root). It requires an `opened` reply for that key and profile, with the
expected cache path and native pid, within 120 s. It then sends
`{"operation":"shutdown","id":2}` and requires a `closed` reply for that
browser, `stopped` for id 2 and exit code 0 within 60 s. `--expect-load` (a
`fixture-inspection` browser) also requires the main-frame load marker, which
shows a renderer ran. `--reuse` sends a second Open of the screen (id 2,
`resume`) and requires the same native browser and window before shutting down
with id 3. The harness prints a JSON summary and keeps it with
`protocol.jsonl`, `browser.log` and `cef-native.log` in the case, which it never
removes. On failure it prints the log tails and ends the process tree
(`taskkill /T` on Windows, the process group on Linux). Processes left after
the exit are counted (by image name on Windows, by process group on Linux) but
do not fail the run.

The CI job `cef-windows` on `windows-latest` builds the product browser with
`--locked` (`CEF_PATH` is `${{ runner.temp }}\cef`, cached by the locked
cef-dll-sys version), runs the harness, then builds the `fixture-inspection`
browser and runs it with `--expect-load --reuse`.

**2026-11-06 local checks (Linux host).** The browser type-checks for
`x86_64-pc-windows-msvc` with `cargo check --features cef/dox`; `dox` skips
cef-dll-sys's build script, which needs the Windows SDK, so nothing was linked
or run for Windows here. On that target the guard stops at its single error.
The Linux build, the guard self-tests and the plugin tests passed, and the
harness passed once against the Linux `fixture-inspection` browser in a private
`xvfb-run` display with `--expect-load --reuse`. Windows evidence comes only
from the CI job.

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
