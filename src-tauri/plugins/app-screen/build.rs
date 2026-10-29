const COMMANDS: &[&str] = &["capabilities", "open", "pin_shortcut"];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .ios_path("ios")
        .build();
}
