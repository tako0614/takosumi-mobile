use serde::{Deserialize, Serialize};

/// The only browsing profile app screens use: one cookie and storage jar shared
/// by every app screen, so a Takosumi OIDC sign-in on the web carries over to
/// every app that signs in with Takosumi.
pub const SHARED_PROFILE: &str = "shared";

const KEY_MAX_LENGTH: usize = 512;
const TITLE_MAX_CHARS: usize = 128;
const URL_MAX_LENGTH: usize = 8 * 1024;
/// Base64 of a 192px PNG is far below this; anything larger is not an icon.
const ICON_PNG_MAX_LENGTH: usize = 512 * 1024;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRequest {
    pub key: String,
    pub url: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_png: Option<String>,
    pub profile: String,
}

impl OpenRequest {
    /// Re-checks the shell's request before any native view is created.
    pub fn validate(&self) -> crate::Result<()> {
        if self.profile != SHARED_PROFILE {
            return Err(crate::Error::InvalidRequest(
                "App screens share one browsing profile.",
            ));
        }
        if self.key.trim().is_empty() || self.key.len() > KEY_MAX_LENGTH {
            return Err(crate::Error::InvalidRequest("App screen key is invalid."));
        }
        if self.title.chars().count() > TITLE_MAX_CHARS {
            return Err(crate::Error::InvalidRequest("App screen title is too long."));
        }
        if self
            .icon_png
            .as_ref()
            .is_some_and(|icon| icon.len() > ICON_PNG_MAX_LENGTH)
        {
            return Err(crate::Error::InvalidRequest("App screen icon is too large."));
        }
        if !is_launchable_url(&self.url) {
            return Err(crate::Error::InvalidRequest(
                "Only https URLs can be opened in an app screen.",
            ));
        }
        Ok(())
    }
}

/// https, a host, and no user info (`https://bank.example@evil.example`).
pub fn is_launchable_url(raw: &str) -> bool {
    if raw.len() > URL_MAX_LENGTH {
        return false;
    }
    match tauri::Url::parse(raw.trim()) {
        Ok(url) => {
            url.scheme() == "https"
                && url.host_str().is_some_and(|host| !host.is_empty())
                && url.username().is_empty()
                && url.password().is_none()
        }
        Err(_) => false,
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitiesResponse {
    pub pin_shortcut: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PinShortcutResponse {
    pub requested: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(url: &str, profile: &str) -> OpenRequest {
        OpenRequest {
            key: "app:cap:web".into(),
            url: url.into(),
            title: "App".into(),
            icon_png: None,
            profile: profile.into(),
        }
    }

    #[test]
    fn only_https_urls_in_the_shared_profile_open() {
        assert!(request("https://app.example/", SHARED_PROFILE).validate().is_ok());
        assert!(request("http://app.example/", SHARED_PROFILE).validate().is_err());
        assert!(request("https://u@app.example/", SHARED_PROFILE).validate().is_err());
        assert!(request("takosumi://app?url=x", SHARED_PROFILE).validate().is_err());
        assert!(request("javascript:alert(1)", SHARED_PROFILE).validate().is_err());
        assert!(request("https://app.example/", "per-app").validate().is_err());
    }
}
