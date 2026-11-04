import { isTauri, invoke } from "@tauri-apps/api/core";
import { platform } from "@tauri-apps/plugin-os";
import { render } from "solid-js/web";
import { webClientSelected, webHomeRequest } from "./web-connections.ts";
import { WebHome } from "./web-home.tsx";
import "./web-home.css";

// Keep the old native client and its saved credentials intact during migration.
// It is never initialized by the Web path (including its native fetch hook).
let storage: Storage | undefined;
try { storage = localStorage; } catch { /* The UI reports saving unavailable. */ }
const web = webClientSelected(location.search, storage);
const legacyNavigation = document.getElementById("legacy-navigation");
if (legacyNavigation) legacyNavigation.hidden = web;
if (!web) {
  legacyNavigation?.querySelector("a")?.addEventListener("click", (event) => {
    event.preventDefault();
    location.replace("?web-client=1");
  });
  void import("./main.tsx");
} else {
  let allowLegacyMobile = false;
  if (isTauri()) {
    try {
      const os = platform();
      allowLegacyMobile = os === "android" || os === "ios";
    } catch { /* An unavailable OS hint must not prevent opening Web home. */ }
  }
  render(() => <WebHome storage={storage} allowLegacyMobile={allowLegacyMobile} open={async (connection) => {
    if (isTauri()) {
      await invoke("plugin:app-screen|open", { payload: webHomeRequest(connection) });
    } else {
      // Browser preview uses an ordinary Web page, with no native capabilities.
      if (connection.profile !== "shared") {
        throw new Error("このブラウザでは、分離されたログイン領域を開けません。端末のTakosumi Appで開いてください。");
      }
      location.assign(`${connection.origin}/`);
    }
  }} />, document.getElementById("root")!);
}
