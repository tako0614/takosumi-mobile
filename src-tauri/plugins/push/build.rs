const COMMANDS: &[&str] = &[
    "permission_state",
    "request_permission",
    "get_token",
    "device_name",
    "create_subscription_keys",
    "delete_subscription_keys",
    "list_subscriptions",
    "take_pending_tap",
    // `addPluginListener` for `notification-tapped`, `notification-received`
    // and `token-refresh`.
    "register_listener",
    "remove_listener",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .ios_path("ios")
        .build();
}
