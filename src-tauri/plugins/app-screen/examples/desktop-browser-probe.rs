#[cfg(target_os = "linux")]
fn main() {
    tauri_plugin_app_screen::run_desktop_browser_probe();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    panic!("This browser operation probe requires the isolated Linux runner.");
}
