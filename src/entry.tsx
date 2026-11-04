import { isTauri, invoke } from "@tauri-apps/api/core";
import { render } from "solid-js/web";
import { webClientSelected, webHomeRequest } from "./web-connections.ts";
import { WebHome } from "./web-home.tsx";
import "./web-home.css";

// Keep the old native client and its saved credentials intact during migration.
// It is never initialized by the Web path (including its native fetch hook).
let storage: Storage | undefined;
try { storage = localStorage; } catch { /* The UI reports saving unavailable. */ }
if (!webClientSelected(location.search, storage)) {
  void import("./main.tsx");
} else {
  render(() => <WebHome storage={storage} open={async (connection) => {
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
