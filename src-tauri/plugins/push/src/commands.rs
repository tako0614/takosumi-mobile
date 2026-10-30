use tauri::{command, AppHandle, Runtime};

use crate::models::{
    CreateKeysRequest, DeviceNameResponse, KeysResponse, PendingTapResponse, PermissionResponse,
    PermissionStateResponse, SubscriptionList, SubscriptionRequest, TokenResponse,
};
use crate::{Result, TakosumiPushExt};

#[command]
pub(crate) async fn permission_state<R: Runtime>(
    app: AppHandle<R>,
) -> Result<PermissionStateResponse> {
    app.takosumi_push().permission_state()
}

#[command]
pub(crate) async fn request_permission<R: Runtime>(
    app: AppHandle<R>,
) -> Result<PermissionResponse> {
    app.takosumi_push().request_permission()
}

#[command]
pub(crate) async fn get_token<R: Runtime>(app: AppHandle<R>) -> Result<TokenResponse> {
    app.takosumi_push().get_token()
}

#[command]
pub(crate) async fn device_name<R: Runtime>(app: AppHandle<R>) -> Result<DeviceNameResponse> {
    app.takosumi_push().device_name()
}

#[command]
pub(crate) async fn create_subscription_keys<R: Runtime>(
    app: AppHandle<R>,
    payload: CreateKeysRequest,
) -> Result<KeysResponse> {
    payload.validate()?;
    app.takosumi_push().create_subscription_keys(payload)
}

#[command]
pub(crate) async fn delete_subscription_keys<R: Runtime>(
    app: AppHandle<R>,
    payload: SubscriptionRequest,
) -> Result<()> {
    payload.validate()?;
    app.takosumi_push().delete_subscription_keys(payload)
}

#[command]
pub(crate) async fn list_subscriptions<R: Runtime>(app: AppHandle<R>) -> Result<SubscriptionList> {
    app.takosumi_push().list_subscriptions()
}

#[command]
pub(crate) async fn take_pending_tap<R: Runtime>(app: AppHandle<R>) -> Result<PendingTapResponse> {
    app.takosumi_push().take_pending_tap()
}
