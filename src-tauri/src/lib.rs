use tauri::Manager;

#[cfg(all(feature = "native-web-home-fixture", target_os = "linux"))]
#[path = "../tests/desktop/web_home.rs"]
mod web_home_fixture;

#[cfg(all(feature = "native-web-home-fixture", not(target_os = "linux")))]
compile_error!("The native Web-home fixture requires Linux; it does not qualify other OSes.");

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .setup(|app| {
            let dir = app.path().app_local_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            app.handle().plugin(
                tauri_plugin_stronghold::Builder::with_argon2(
                    &dir.join("takosumi-mobile-stronghold-salt.bin"),
                )
                .build(),
            )?;
            #[cfg(mobile)]
            app.handle().plugin(tauri_plugin_barcode_scanner::init())?;
            #[cfg(mobile)]
            app.handle().plugin(tauri_plugin_biometric::init())?;
            #[cfg(mobile)]
            app.handle().plugin(tauri_plugin_keystore::init())?;
            app.handle().plugin(tauri_plugin_app_screen::init())?;
            #[cfg(mobile)]
            app.handle().plugin(tauri_plugin_takosumi_push::init())?;
            #[cfg(all(feature = "native-web-home-fixture", target_os = "linux"))]
            web_home_fixture::start(app.handle().clone());
            Ok(())
        });
    #[cfg(all(feature = "native-web-home-fixture", target_os = "linux"))]
    web_home_fixture::run(builder, tauri::generate_context!());
    #[cfg(not(feature = "native-web-home-fixture"))]
    builder
        .run(tauri::generate_context!())
        .expect("error while running Takosumi mobile");
}
