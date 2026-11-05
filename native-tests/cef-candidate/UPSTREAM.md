# Browser-foundation candidate

The minimal CEF process/client lifecycle follows `cefsimple` from
[cef-rs commit 2861f699](https://github.com/tauri-apps/cef-rs/tree/2861f6991fae3ee31f877891382adcbc9b9f0953/examples/cefsimple).
It is used under the MIT option; see `LICENSE-UPSTREAM`.

The crate pins the same CEF ABI as the evaluated Tauri 3 alpha runtime. This is
a standalone Linux native candidate, excluded from ordinary App builds. It
reuses the product's request validator without enabling the Wry adapter. The
product's Tauri UI, installed profiles and runtime selection are unchanged.

Use a separate Cargo target and an owned `CEF_PATH` under `/root/dev`. The first
build needs the CEF SDK. Never use the product's target directory or change the
shared OS to satisfy this experiment. Keep the generated lockfile for the exact
dependency set. Native build/runtime evidence is separate from portable checks.

The runner must create a fresh private case, UUID marker, validated request,
Downloads and XDG directories. Only the two anonymous public app roots are
accepted. Do not use real accounts, cloud save, operator data or credentials.
The root test process uses an unsandboxed browser and a private debug endpoint;
it cannot qualify sandboxing, distribution or an end-user browser configuration.

The native client does not install download/permission handlers, site Web API
replacements or a native IPC bridge. Measurement settings are explicit in the
driver: `--files cdp` uses a headless browser, supplies actual existing file
inputs through CDP and sets `Browser.setDownloadBehavior(allowAndName)` to an
owned path. This is a File API / Blob-download component check. It does not
qualify an OS chooser or the default Save UI. `--files native-picker` instead
opens a visible browser in a fresh Xvfb session for Excalidraw's actual
File System Access Open and Save As choosers, preserving the native site-write
warning. It is a separate experiment, not a fallback that
turns a failed CDP operation into success. Its download behavior is still the
same explicit CDP setting, but the File System Access save does not use that
download service. It selects a previously absent owned output and requires
the exact file with the imported nonce and keyboard edit. The English GTK
confirm-pixel heuristic is scoped to the inspected theme/geometry.

Actual bytes and normal native shutdown are required for file-operation success.
The separate two-profile modes use the same native factory with two request
contexts, native main-frame/CDP page identity and actual native close receipts.
They test synthetic same-origin storage/cookies and a CDP-driven standard popup;
cookies are set through CDP, not obtained from a provider. The recorder ends only
after SDK shutdown and owned client release, independently of remaining Sender
handles. Strict process/guard checks still gate every success flag.

Tauri UI integration, real login/cross-origin popup return, site permissions,
Web Push, restart persistence, sandboxing, other OSes and distribution remain
separate unfinished checks. These experiments do not select a product engine.

The permission modes reuse Chrome's default prompt, with no native permission
handler or CDP grant. They place a uniquely named, PID-verified owned window in
the inspected English Linux geometry and click a single Allow/Block choice.
The two known prompt layouts are test-fixture details. Actual pending request,
secure origin, resolved permission and the other profile's state must agree;
five replay controls reject false evidence before clicking. Excalidraw passed
this notification-permission component; Photopea's full final flow remains
unqualified. API notification events do not prove OS display or Push: the
earlier owned D-Bus session lacked its notification service, and an unattributed
allocator diagnostic also prevents claiming desktop-service stability.

The separate `site-notifications` fixture explicitly starts a supplied real
Dunst 1.12.2 in a fresh owned session, attests D-Bus/XRes ownership and records
native input. It does not install a notification daemon on the shared OS or
implement an App push service. Actual same-origin/same-tag alerts in two native
profiles failed: Excalidraw routed Work's action to Personal; Photopea closed
Work's page object when Personal created its alert. Normal shutdown does not
turn those functional failures into success. Native notification UI, profile
isolation at that boundary, window restoration and network Push remain
unqualified. The fixture does not rewrite page tags to avoid this failure.

The matching Chromium 154.0.8037.98 source supports the collision hypothesis:
[Blink tag/token](https://github.com/chromium/chromium/blob/154.0.8037.98/third_party/blink/renderer/modules/notifications/notification.cc),
[origin/token ID](https://github.com/chromium/chromium/blob/154.0.8037.98/content/browser/notifications/notification_id_generator.cc),
[singleton listener registration](https://github.com/chromium/chromium/blob/154.0.8037.98/content/browser/notifications/notification_event_dispatcher_impl.cc).
These are source references for diagnosis, not copied product code or a
qualified engine patch. Persistent Service Worker notifications use a different
path and were not tested here.
