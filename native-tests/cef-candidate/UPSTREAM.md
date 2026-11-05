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
opens a visible browser in a fresh Xvfb session to inspect Excalidraw's actual
File System Access chooser. It is a separate experiment, not a fallback that
turns a failed CDP operation into success. Its download behavior is still the
same explicit CDP setting.

Actual bytes and normal native shutdown are required for operation success.
A verified single UUID context does not qualify two-profile data/session
isolation. Tauri UI integration, popup/login, site permissions, Web Push,
sandboxing, other OSes and distribution remain separate unfinished checks.
