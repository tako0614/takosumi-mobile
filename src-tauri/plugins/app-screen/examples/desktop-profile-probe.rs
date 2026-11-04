#[cfg(target_os = "linux")]
fn main() {
    tauri_plugin_app_screen::run_desktop_profile_fixture();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    panic!("This synthetic GTK runtime fixture requires Linux; it does not qualify other OSes.");
}
