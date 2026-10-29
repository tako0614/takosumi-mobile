//! Native app screens for Takosumi Mobile: each installed app opens in its own
//! WKWebView / Android WebView, separate from the Tauri shell's webview, with a
//! thin native top bar that always shows the current host.

#[cfg(mobile)]
use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

pub use models::*;

mod error;
mod models;

#[cfg(mobile)]
mod commands;
#[cfg(mobile)]
mod mobile;

pub use error::{Error, Result};
#[cfg(mobile)]
use mobile::AppScreen;

#[cfg(mobile)]
pub trait AppScreenExt<R: Runtime> {
    fn app_screen(&self) -> &AppScreen<R>;
}

#[cfg(mobile)]
impl<R: Runtime, T: Manager<R>> AppScreenExt<R> for T {
    fn app_screen(&self) -> &AppScreen<R> {
        self.state::<AppScreen<R>>().inner()
    }
}

#[cfg(mobile)]
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("app-screen")
        .invoke_handler(tauri::generate_handler![
            commands::capabilities,
            commands::open,
            commands::pin_shortcut
        ])
        .setup(|app, api| {
            let app_screen = mobile::init(app, api)?;
            app.manage(app_screen);
            Ok(())
        })
        .build()
}
