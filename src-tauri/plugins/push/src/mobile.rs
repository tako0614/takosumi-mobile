use serde::de::DeserializeOwned;
use tauri::{
    plugin::{PluginApi, PluginHandle},
    AppHandle, Runtime,
};

use crate::models::{
    CreateKeysRequest, DeviceNameResponse, KeysResponse, PendingTapResponse, PermissionResponse,
    PermissionStateResponse, SubscriptionList, SubscriptionRequest, TokenResponse,
};

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_takosumi_push);

pub fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> crate::Result<TakosumiPush<R>> {
    #[cfg(target_os = "android")]
    let handle = api.register_android_plugin("com.takosumi.push", "PushPlugin")?;
    #[cfg(target_os = "ios")]
    let handle = api.register_ios_plugin(init_plugin_takosumi_push)?;
    Ok(TakosumiPush(handle))
}

pub struct TakosumiPush<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> TakosumiPush<R> {
    pub fn permission_state(&self) -> crate::Result<PermissionStateResponse> {
        self.0
            .run_mobile_plugin("permissionState", ())
            .map_err(Into::into)
    }

    pub fn request_permission(&self) -> crate::Result<PermissionResponse> {
        self.0
            .run_mobile_plugin("requestPermission", ())
            .map_err(Into::into)
    }

    pub fn get_token(&self) -> crate::Result<TokenResponse> {
        self.0.run_mobile_plugin("getToken", ()).map_err(Into::into)
    }

    pub fn device_name(&self) -> crate::Result<DeviceNameResponse> {
        self.0
            .run_mobile_plugin("deviceName", ())
            .map_err(Into::into)
    }

    pub fn create_subscription_keys(
        &self,
        payload: CreateKeysRequest,
    ) -> crate::Result<KeysResponse> {
        self.0
            .run_mobile_plugin("createSubscriptionKeys", payload)
            .map_err(Into::into)
    }

    pub fn delete_subscription_keys(&self, payload: SubscriptionRequest) -> crate::Result<()> {
        self.0
            .run_mobile_plugin("deleteSubscriptionKeys", payload)
            .map_err(Into::into)
    }

    pub fn list_subscriptions(&self) -> crate::Result<SubscriptionList> {
        self.0
            .run_mobile_plugin("listSubscriptions", ())
            .map_err(Into::into)
    }

    pub fn take_pending_tap(&self) -> crate::Result<PendingTapResponse> {
        self.0
            .run_mobile_plugin("takePendingTap", ())
            .map_err(Into::into)
    }
}
