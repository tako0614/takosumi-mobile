//! App screens show other people's web pages in plain native webviews, which
//! Tauri never sees. Tauri grants IPC only through capabilities, and refuses
//! remote content unless a capability names it in `remote`. These tests read
//! the app's capability files with Tauri's own parser and check that only the
//! shell's bundled `main` window can call this plugin's commands.

use std::path::PathBuf;

use tauri_utils::acl::capability::Capability;

fn capabilities() -> Vec<(String, Capability)> {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../capabilities");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&directory).expect("capabilities directory") {
        let path = entry.expect("capability file").path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.ends_with(".json"), "unexpected capability file {name}");
        let text = std::fs::read_to_string(&path).expect("capability text");
        let capability: Capability = serde_json::from_str(&text).expect("capability parses");
        found.push((name, capability));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

fn identifiers(capability: &Capability) -> Vec<String> {
    capability
        .permissions
        .iter()
        .map(|entry| entry.identifier().get().to_string())
        .collect()
}

#[test]
fn no_capability_reaches_remote_pages_or_other_webviews() {
    let all = capabilities();
    assert_eq!(
        all.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
        ["default.json", "mobile.json"]
    );
    for (name, capability) in &all {
        assert!(capability.remote.is_none(), "{name} grants IPC to remote URLs");
        assert!(capability.local, "{name} must apply to bundled content only");
        assert_eq!(capability.windows, ["main"], "{name}");
        assert!(capability.webviews.is_empty(), "{name}");
    }
}

#[test]
fn only_the_mobile_shell_capability_grants_app_screen_commands() {
    for (name, capability) in capabilities() {
        let grants = identifiers(&capability)
            .into_iter()
            .filter(|identifier| identifier.starts_with("app-screen:"))
            .collect::<Vec<_>>();
        if name == "mobile.json" {
            assert_eq!(grants, ["app-screen:default"]);
        } else {
            assert!(grants.is_empty(), "{name} grants {grants:?}");
        }
    }
}
