const COMMANDS: &[&str] = &[
    "capabilities",
    "open",
    "pin_shortcut",
    "open_client",
    "open_browser_view",
    "take_pending_action",
    // `addPluginListener` for the bar's `actionPending` event.
    "register_listener",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .ios_path("ios")
        .build();
}
