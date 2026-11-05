# Standalone browser-foundation candidate

Experimental Linux CEF 154 process using the App's production `OpenRequest`
validator and UUID profile IDs. It does not replace the Tauri runtime or change
installed App profiles. See [upstream attribution and scope](UPSTREAM.md).

Build with an owned SDK/target directory and the committed candidate lockfile:

```bash
CEF_PATH=/absolute/owned/sdk CARGO_TARGET_DIR=/absolute/owned/target cargo build --locked --manifest-path native-tests/cef-candidate/Cargo.toml
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

Default `--files cdp` uses a headless browser. It supplies the site's actual
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

On 2026-11-05 the basic same-origin profile test and the synthetic popup flow
passed on native CEF, including normal closure. Earlier popup runs passed the
functional assertions but failed shutdown and remain failures. Milestones
located a recorder EOF wait after SDK shutdown/client release; the recorder now
ends on the explicit runtime-complete boundary and is still joined. This does
not establish that all retained SDK references or cold-start issues are solved.

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
persistence or real authentication. Tauri UI integration, real login/popup
return, site permission choices, Web Push delivery, sandboxing, other OSes,
updates and distribution remain unfinished. The runner
uses `no_sandbox` in its disposable root test environment; it is not a release
configuration or an engine-selection decision.
