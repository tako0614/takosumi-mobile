//! Fixture-only macOS home store. Changing the Tauri identifier alone does
//! not select a WKWebsiteDataStore. Select an explicit store before the
//! main view is built, and refuse the OS versions where Wry falls back to
//! the default store. The runner retains this UUID across this case's launches.
//! Pinned tauri-runtime 2.11.3 drops data_store_identifier in From<WindowConfig>,
//! so the configured window must be created through the explicit builder API.
use super::{owned_case, Result};

pub(super) fn prepare(context: &mut tauri::Context<tauri::Wry>) -> Result<()> {
    let case = owned_case()?;
    let version = std::process::Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .map_err(|error| error.to_string())?;
    let version_text = String::from_utf8(version.stdout).map_err(|error| error.to_string())?;
    if !version.status.success()
        || !version_text
            .trim()
            .split('.')
            .next()
            .and_then(|major| major.parse::<u32>().ok())
            .is_some_and(|major| major >= 14)
    {
        return Err("The Mac fixture requires macOS 14+ for an isolated WKWebView store".into());
    }
    let store: [u8; 16] = serde_json::from_slice(
        &std::fs::read(case.join("home-store.json")).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if store == [0; 16] {
        return Err("The Mac fixture needs a nonzero owned home store UUID".into());
    }
    let config = context.config_mut();
    if config.app.windows.len() != 1 || config.app.windows[0].label != "main" {
        return Err("The Mac fixture expects exactly the main home window".into());
    }
    let identifier = store
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    config.identifier = format!("com.takosumi.fixture.browserprocess.m3{identifier}");
    config.app.windows[0].data_store_identifier = Some(store);
    config.app.windows[0].incognito = false;
    config.app.windows[0].create = false;
    let phase = std::env::var("TAKOSUMI_BROWSER_PROCESS_PHASE").unwrap_or_else(|_| "first".into());
    if !["first", "restart", "reset"].contains(&phase.as_str()) {
        return Err("Unknown Mac fixture phase".into());
    }
    std::fs::write(
        case.join(format!("app-home-store-{phase}.json")),
        serde_json::to_vec_pretty(&serde_json::json!({
            "identifier":config.identifier,"store":store,"macOS":version_text.trim(),
            "appPid":std::process::id(),
            "scope":"requested persistent WKWebView store; explicit builder and separate-case/restart checks required"
        }))
        .unwrap(),
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

/// Called on the main thread during App setup, before the fixture starts.
pub(crate) fn create_home(app: &tauri::App) -> Result<()> {
    use tauri::Manager;
    let config = &app.config().app.windows[0];
    if config.create || config.incognito || app.get_webview_window("main").is_some() {
        return Err("The Mac fixture must create its isolated home exactly once".into());
    }
    let store = config
        .data_store_identifier
        .ok_or("Missing Mac fixture home store")?;
    tauri::WebviewWindowBuilder::from_config(app, config)
        .map_err(|error| error.to_string())?
        .data_store_identifier(store)
        .build()
        .map_err(|error| error.to_string())?;
    Ok(())
}
