use serde::de::DeserializeOwned;
use tauri::{
    plugin::{PluginApi, PluginHandle},
    AppHandle, Runtime,
};

use crate::models::{CapabilitiesResponse, OpenRequest, PinShortcutResponse};

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_app_screen);

pub fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> crate::Result<AppScreen<R>> {
    #[cfg(target_os = "android")]
    let handle = api.register_android_plugin("com.takosumi.appscreen", "AppScreenPlugin")?;
    #[cfg(target_os = "ios")]
    let handle = api.register_ios_plugin(init_plugin_app_screen)?;
    Ok(AppScreen(handle))
}

pub struct AppScreen<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> AppScreen<R> {
    pub fn capabilities(&self) -> crate::Result<CapabilitiesResponse> {
        self.0
            .run_mobile_plugin("capabilities", ())
            .map_err(Into::into)
    }

    pub fn open(&self, payload: OpenRequest) -> crate::Result<()> {
        self.0
            .run_mobile_plugin("open", payload)
            .map_err(Into::into)
    }

    pub fn pin_shortcut(&self, payload: OpenRequest) -> crate::Result<PinShortcutResponse> {
        self.0
            .run_mobile_plugin("pinShortcut", payload)
            .map_err(Into::into)
    }
}
