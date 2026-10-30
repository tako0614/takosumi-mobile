use tauri::{command, AppHandle, Runtime};

use crate::models::{
    BrowserViewRequest, BrowserViewResponse, CapabilitiesResponse, OpenClientRequest,
    OpenClientResponse, OpenRequest, OpenWebRequest, PendingActionResponse, PinShortcutResponse,
};
use crate::{AppScreenExt, Result};

#[command]
pub(crate) async fn capabilities<R: Runtime>(app: AppHandle<R>) -> Result<CapabilitiesResponse> {
    app.app_screen().capabilities()
}

#[command]
pub(crate) async fn open<R: Runtime>(app: AppHandle<R>, payload: OpenRequest) -> Result<()> {
    payload.validate()?;
    app.app_screen().open(payload)
}

#[command]
pub(crate) async fn pin_shortcut<R: Runtime>(
    app: AppHandle<R>,
    payload: OpenRequest,
) -> Result<PinShortcutResponse> {
    payload.validate()?;
    app.app_screen().pin_shortcut(payload)
}

#[command]
pub(crate) async fn open_client<R: Runtime>(
    app: AppHandle<R>,
    payload: OpenClientRequest,
) -> Result<OpenClientResponse> {
    payload.validate()?;
    app.app_screen().open_client(payload)
}

#[command]
pub(crate) async fn open_browser_view<R: Runtime>(
    app: AppHandle<R>,
    payload: BrowserViewRequest,
) -> Result<BrowserViewResponse> {
    payload.validate()?;
    app.app_screen().open_browser_view(payload)
}

#[command]
pub(crate) async fn open_web<R: Runtime>(
    app: AppHandle<R>,
    payload: OpenWebRequest,
) -> Result<BrowserViewResponse> {
    payload.validate()?;
    app.app_screen().open_web(payload)
}

#[command]
pub(crate) async fn take_pending_action<R: Runtime>(
    app: AppHandle<R>,
) -> Result<PendingActionResponse> {
    app.app_screen().take_pending_action()
}
