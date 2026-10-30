//! Takosumi Mobile's own notifications (TASK-0119 step 7). Each Takosumi
//! instance the person turns notifications on for gets one subscription on the
//! push relay (`relay/`) and one P-256 key pair plus a 16-byte `auth` here. The
//! instance sends standard Web Push (RFC 8291 `aes128gcm`) to the relay, the
//! relay forwards `{s, b}` through APNs/FCM, and the native side decrypts and
//! shows the Declarative Web Push message on that instance's channel.
//!
//! Private keys never reach JavaScript: Android keeps them wrapped by an
//! Android Keystore key, iOS in the Keychain under an access group a
//! Notification Service Extension can share. JavaScript only learns the public
//! half (`p256dh`, `auth`) to register with the instance.

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
use mobile::TakosumiPush;

#[cfg(mobile)]
pub trait TakosumiPushExt<R: Runtime> {
    fn takosumi_push(&self) -> &TakosumiPush<R>;
}

#[cfg(mobile)]
impl<R: Runtime, T: Manager<R>> TakosumiPushExt<R> for T {
    fn takosumi_push(&self) -> &TakosumiPush<R> {
        self.state::<TakosumiPush<R>>().inner()
    }
}

#[cfg(mobile)]
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("takosumi-push")
        .invoke_handler(tauri::generate_handler![
            commands::permission_state,
            commands::request_permission,
            commands::get_token,
            commands::device_name,
            commands::create_subscription_keys,
            commands::delete_subscription_keys,
            commands::list_subscriptions,
            commands::take_pending_tap
        ])
        .setup(|app, api| {
            let push = mobile::init(app, api)?;
            app.manage(push);
            Ok(())
        })
        .build()
}
