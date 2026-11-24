use tauri::Manager;

#[cfg(windows)]
mod windows_exit;

#[cfg(all(debug_assertions, any(target_os = "ios", target_os = "android")))]
mod bootstrap_fixture;

// The browser-process fixture drives the same flow on Linux (GTK WebKit home)
// Windows (WebView2) and macOS (WKWebView); see tests/desktop/browser_process.rs.
#[cfg(all(
    feature = "native-browser-process-fixture",
    any(target_os = "linux", windows, target_os = "macos")
))]
#[path = "../tests/desktop/browser_process.rs"]
mod browser_process_fixture;
#[cfg(all(feature = "native-web-home-fixture", target_os = "linux"))]
#[path = "../tests/desktop/web_home.rs"]
mod web_home_fixture;
#[cfg(all(
    feature = "native-browser-process-fixture",
    not(any(target_os = "linux", windows, target_os = "macos"))
))]
compile_error!("The browser-process App fixture requires Linux, Windows or macOS.");
#[cfg(all(
    feature = "native-browser-process-fixture",
    feature = "native-web-home-fixture"
))]
compile_error!("Select one actual-App fixture at a time.");

#[cfg(all(feature = "native-web-home-fixture", not(target_os = "linux")))]
compile_error!("The native Web-home fixture requires Linux; it does not qualify other OSes.");
// All three desktop platforms build the same adapter; macOS App packaging
// and distribution remain separate from the explicit native fixture.
#[cfg(all(
    feature = "desktop-cef",
    not(any(target_os = "linux", windows, target_os = "macos"))
))]
compile_error!(
    "The per-profile CEF desktop browser is available on Linux, Windows and macOS only."
);
#[cfg(all(feature = "desktop-cef", feature = "native-web-home-fixture"))]
compile_error!("The native Web-home fixture drives the Wry adapter; build it without desktop-cef.");

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
            #[cfg(all(debug_assertions, target_os = "ios"))]
            bootstrap_fixture::install(app);
            #[cfg(all(debug_assertions, target_os = "android"))]
            bootstrap_fixture::install_android(app);
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
            #[cfg(all(feature = "native-browser-process-fixture", target_os = "macos"))]
            browser_process_fixture::create_home(app).map_err(std::io::Error::other)?;
            #[cfg(all(feature = "native-web-home-fixture", target_os = "linux"))]
            web_home_fixture::start(app.handle().clone());
            #[cfg(all(
                feature = "native-browser-process-fixture",
                any(target_os = "linux", windows, target_os = "macos")
            ))]
            browser_process_fixture::start(app.handle().clone());
            Ok(())
        });
    #[cfg(all(debug_assertions, target_os = "android"))]
    let builder = builder.on_page_load(bootstrap_fixture::observe_android_page_load);
    #[cfg(all(feature = "native-web-home-fixture", target_os = "linux"))]
    web_home_fixture::run(builder, tauri::generate_context!());
    #[cfg(all(
        feature = "native-browser-process-fixture",
        any(target_os = "linux", windows, target_os = "macos")
    ))]
    browser_process_fixture::run(builder, tauri::generate_context!());
    #[cfg(not(any(
        feature = "native-web-home-fixture",
        feature = "native-browser-process-fixture"
    )))]
    {
        let app = builder
            .build(tauri::generate_context!())
            .expect("error while running Takosumi mobile");
        #[cfg(windows)]
        {
            // Returning from the loop does not release managed WebViews
            // still retained by AppHandles. Close our views on the UI thread
            // only once exit is terminal; a cancelled request keeps them live.
            let close_failures = std::rc::Rc::new(std::cell::Cell::new(0usize));
            let failures_in_run = close_failures.clone();
            let code = app.run_return(move |app, event| {
                let failed = windows_exit::close_managed_views(
                    matches!(event, tauri::RunEvent::Exit),
                    || app.webviews().into_values(),
                    |view| {
                        // Preserve the per-view cleanup that Tauri would run
                        // after this callback, before removing its registry row.
                        drop(std::mem::take(&mut *view.resources_table()));
                        view.close()
                    },
                );
                failures_in_run.set(failures_in_run.get() + failed);
            });
            let failed = close_failures.get();
            std::process::exit(windows_exit::exit_code(code, failed));
        }
        #[cfg(not(windows))]
        app.run(|_, _| {});
    }
}
