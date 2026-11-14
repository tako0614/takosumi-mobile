import { isTauri, invoke } from "@tauri-apps/api/core";
import { platform } from "@tauri-apps/plugin-os";
import { render } from "solid-js/web";
import { createSignal } from "solid-js";
import { webClientSelected } from "./web-connections.ts";
import { WebHome } from "./web-home.tsx";
import { clearWebProfileData } from "./web-profile-data.ts";
import { createWebPermissionPrompts, type WebPermissionPrompts } from "./web-permissions.ts";
import { openWebConnection, recoverWebProfile } from "./web-profile-recovery.ts";
import { BOOTSTRAP_EVENT, createBootstrapObserver } from "./fixture-bootstrap.ts";
import "./web-home.css";

// Static dependencies have already evaluated. Times stay in this document's
// browser Performance domain; neither this event nor a JSX ref proves paint/AX.
const observeBootstrap = createBootstrapObserver({
  enabled: (import.meta as ImportMeta & { readonly env?: Readonly<Record<string, unknown>> })
    .env?.VITE_TAKOSUMI_FIXTURE_BOOTSTRAP === "1",
  send: (payload) => invoke("plugin:event|emit", { event: BOOTSTRAP_EVENT, payload }),
  now: () => performance.now(),
});
observeBootstrap("entry");

// Keep the old native client and its saved credentials intact during migration.
// It is never initialized by the Web path (including its native fetch hook).
let storage: Storage | undefined;
observeBootstrap("storage_begin");
try {
  storage = localStorage;
  observeBootstrap("storage_end");
} catch {
  /* The UI reports saving unavailable. */
  observeBootstrap("storage_unavailable");
}
observeBootstrap("selection_begin");
const web = webClientSelected(location.search, storage);
observeBootstrap("selection_end");
const legacyNavigation = document.getElementById("legacy-navigation");
if (legacyNavigation) legacyNavigation.hidden = web;
if (!web) {
  observeBootstrap("legacy");
  legacyNavigation?.querySelector("a")?.addEventListener("click", (event) => {
    event.preventDefault();
    location.replace("?web-client=1");
  });
  void import("./main.tsx");
} else {
  let allowLegacyMobile = false;
  const [canClearData, setCanClearData] = createSignal(false);
  const [canRecover, setCanRecover] = createSignal(false);
  const [permissionPrompts, setPermissionPrompts] = createSignal<WebPermissionPrompts>();
  if (isTauri()) {
    void invoke<unknown>("plugin:app-screen|capabilities").then((value) => {
      setCanClearData(typeof value === "object" && value !== null &&
        (value as Record<string, unknown>).clearProfile === true);
      setCanRecover(typeof value === "object" && value !== null &&
        (value as Record<string, unknown>).recoverProfile === true);
      if (typeof value === "object" && value !== null &&
        (value as Record<string, unknown>).permissionPrompts === true) {
        setPermissionPrompts(createWebPermissionPrompts(invoke));
      }
    }).catch(() => { /* Unsupported engines must not advertise data deletion. */ });
    try {
      const os = platform();
      allowLegacyMobile = os === "android" || os === "ios";
    } catch { /* An unavailable OS hint must not prevent opening Web home. */ }
  }
  render(() => <WebHome storage={storage} allowLegacyMobile={allowLegacyMobile} observeBootstrap={observeBootstrap}
    clearData={canClearData() ? (connection) => clearWebProfileData(connection, invoke) : undefined}
    recover={canRecover() ? (connection, ticket) => recoverWebProfile(connection, ticket, invoke) : undefined}
    permissionPrompts={permissionPrompts()}
    open={async (connection) => {
    if (isTauri()) {
      await openWebConnection(connection, invoke);
    } else {
      // Browser preview uses an ordinary Web page, with no native capabilities.
      if (connection.profile !== "shared") {
        throw new Error("このブラウザでは、分離されたログイン領域を開けません。端末のTakosumi Appで開いてください。");
      }
      location.assign(`${connection.origin}/`);
    }
  }} />, document.getElementById("root")!);
  observeBootstrap("render_returned");
}
