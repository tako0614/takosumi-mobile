use tauri::{command, AppHandle, Runtime};

use crate::models::{CapabilitiesResponse, OpenRequest, PinShortcutResponse};
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
