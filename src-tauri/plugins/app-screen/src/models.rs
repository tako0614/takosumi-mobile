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
const SCOPE_MAX_LENGTH: usize = 512;
const CLIENT_NAME_MAX_CHARS: usize = 64;
const CLIENT_LINK_MAX_LENGTH: usize = 2_048;
const CLIENT_IDENTIFIER_MAX_LENGTH: usize = 155;

/// Takosumi Mobile's own application id; a declared native app is never it.
const OWN_APP_ID: &str = "com.takosumi";

/// Hosts the Tauri shell serves its own UI and IPC from; never an app screen.
pub const SHELL_HOSTS: &[&str] = &["tauri.localhost", "ipc.localhost", "asset.localhost"];

/// Custom schemes a native app link may not use. Mirror of
/// `INTERFACE_CLIENT_BLOCKED_SCHEMES` in takosumi `contract/interface-clients.ts`
/// (tests/app-screen-isolation.test.ts keeps them equal).
pub const BLOCKED_CLIENT_SCHEMES: &[&str] = &[
    "http",
    "https",
    "javascript",
    "vbscript",
    "data",
    "file",
    "filesystem",
    "blob",
    "about",
    "content",
    "intent",
    "android-app",
    "takosumi",
    "tel",
    "callto",
    "sms",
    "smsto",
    "mms",
    "mmsto",
    "mailto",
    "facetime",
    "facetime-audio",
    "facetime-prompt",
    "facetime-audio-prompt",
    "itms",
    "itmss",
    "itms-apps",
    "itms-appss",
    "itms-services",
    "itms-beta",
    "market",
    "app-settings",
    "app-prefs",
    "prefs",
    "shortcuts",
    "x-web-search",
    "x-safari-http",
    "x-safari-https",
    "googlechrome",
    "googlechromes",
    "googlechrome-x-callback",
    "firefox",
    "firefox-focus",
    "focus",
    "microsoft-edge",
    "microsoft-edge-http",
    "microsoft-edge-https",
    "opera",
    "opera-http",
    "opera-https",
    "touch-http",
    "touch-https",
    "brave",
    "duckduckgo",
    "ddgquicklink",
    "samsunginternet",
    "yandexbrowser-open-url",
    "firefox-klar",
    "onionhttp",
    "onionhttps",
    "ws",
    "wss",
    "ftp",
];

/// The declared native app an app screen's bar offers ("{name}で開く").
/// A verified one carries its links; an unverified one carries none, and the
/// bar sends the person to the shell's chooser, which asks first.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BannerClient {
    pub name: String,
    pub verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub universal_link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRequest {
    pub key: String,
    pub url: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_png: Option<String>,
    pub profile: String,
    /// The instance's Accounts origin; with the app's own origin it is all the
    /// screen shows in its main frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accounts_origin: Option<String>,
    /// (instance, account) the last page is remembered for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default)]
    pub resume: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<BannerClient>,
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
        if self
            .accounts_origin
            .as_deref()
            .is_some_and(|origin| !is_https_origin(origin))
        {
            return Err(crate::Error::InvalidRequest(
                "The Accounts origin must be an https origin.",
            ));
        }
        if self.scope.as_deref().is_some_and(|scope| !is_valid_scope(scope)) {
            return Err(crate::Error::InvalidRequest("App screen scope is invalid."));
        }
        if let Some(client) = &self.client {
            client.validate()?;
        }
        Ok(())
    }
}

impl BannerClient {
    pub fn validate(&self) -> crate::Result<()> {
        if !is_client_name(&self.name) {
            return Err(crate::Error::InvalidRequest("Native app name is invalid."));
        }
        let has_links = self.package.is_some()
            || self.app_link.is_some()
            || self.universal_link.is_some()
            || self.open.is_some();
        if !self.verified {
            // An unverified app opens only from the chooser, after asking.
            return if has_links {
                Err(crate::Error::InvalidRequest(
                    "An unverified native app carries no links.",
                ))
            } else {
                Ok(())
            };
        }
        check_links(
            self.package.as_deref(),
            self.app_link.as_deref(),
            self.universal_link.as_deref(),
            self.open.as_deref(),
        )
    }
}

/// `open_client`: hand the screen to its declared native app.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenClientRequest {
    pub platform: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub universal_link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<String>,
}

impl OpenClientRequest {
    pub fn validate(&self) -> crate::Result<()> {
        match self.platform.as_str() {
            "android" => {
                if self.universal_link.is_some() || self.package.is_none() {
                    return Err(crate::Error::InvalidRequest(
                        "An Android native app needs its package and no universal link.",
                    ));
                }
            }
            "ios" => {
                if self.package.is_some() || self.app_link.is_some() {
                    return Err(crate::Error::InvalidRequest(
                        "An iOS native app takes a universal link or a custom scheme.",
                    ));
                }
            }
            _ => return Err(crate::Error::InvalidRequest("Unknown platform.")),
        }
        check_links(
            self.package.as_deref(),
            self.app_link.as_deref(),
            self.universal_link.as_deref(),
            self.open.as_deref(),
        )
    }
}

fn check_links(
    package: Option<&str>,
    app_link: Option<&str>,
    universal_link: Option<&str>,
    open: Option<&str>,
) -> crate::Result<()> {
    if app_link.is_none() && universal_link.is_none() && open.is_none() {
        return Err(crate::Error::InvalidRequest("A native app needs a link."));
    }
    if package.is_some_and(|package| !is_android_package(package)) {
        return Err(crate::Error::InvalidRequest("Android package is invalid."));
    }
    if app_link.is_some() && package.is_none() {
        return Err(crate::Error::InvalidRequest(
            "An App Link opens only with its package.",
        ));
    }
    for link in [app_link, universal_link].into_iter().flatten() {
        if !is_https_link(link) {
            return Err(crate::Error::InvalidRequest(
                "Native app links must be https.",
            ));
        }
    }
    if open.is_some_and(|open| !is_custom_scheme_link(open)) {
        return Err(crate::Error::InvalidRequest(
            "This custom scheme cannot be opened.",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenClientResponse {
    /// `opened` or `not_installed`.
    pub result: String,
}

/// `open_browser_view`: Custom Tabs on Android, `SFSafariViewController` on iOS.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserViewRequest {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolbar_color: Option<String>,
}

impl BrowserViewRequest {
    pub fn validate(&self) -> crate::Result<()> {
        if !is_launchable_url(&self.url) {
            return Err(crate::Error::InvalidRequest(
                "Only https URLs can be opened in the browser view.",
            ));
        }
        if self
            .toolbar_color
            .as_deref()
            .is_some_and(|color| !is_hex_color(color))
        {
            return Err(crate::Error::InvalidRequest("Toolbar color is invalid."));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserViewResponse {
    /// `browser-view`, or `browser` when no Custom Tabs browser is installed.
    pub result: String,
}

/// `take_pending_action`: what an app screen's bar left for the shell.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingActionResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<PendingAction>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingAction {
    pub kind: String,
    pub key: String,
    pub url: String,
    pub reason: String,
}

/// https, a host, no user info (`https://bank.example@evil.example`), and not
/// one of the Tauri shell's own hosts.
pub fn is_launchable_url(raw: &str) -> bool {
    if raw.len() > URL_MAX_LENGTH {
        return false;
    }
    match tauri::Url::parse(raw.trim()) {
        Ok(url) => {
            url.scheme() == "https"
                && url
                    .host_str()
                    .is_some_and(|host| !host.is_empty() && !is_shell_host(host))
                && url.username().is_empty()
                && url.password().is_none()
        }
        Err(_) => false,
    }
}

fn is_shell_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    SHELL_HOSTS.contains(&host.as_str())
}

/// Exactly an https origin (`https://host[:port]`), as `URL.origin` writes it.
pub fn is_https_origin(raw: &str) -> bool {
    match tauri::Url::parse(raw) {
        Ok(url) => {
            url.scheme() == "https"
                && is_launchable_url(raw)
                && url.origin().ascii_serialization() == raw
        }
        Err(_) => false,
    }
}

fn is_valid_scope(scope: &str) -> bool {
    !scope.is_empty()
        && scope.len() <= SCOPE_MAX_LENGTH
        && !scope.chars().any(|character| character.is_control())
}

fn is_client_name(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty()
        && trimmed.chars().count() <= CLIENT_NAME_MAX_CHARS
        && !trimmed.chars().any(|character| {
            character.is_control()
                || matches!(character, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
        })
}

/// An Android applicationId (`com.example.app`), never Takosumi Mobile's own.
pub fn is_android_package(raw: &str) -> bool {
    if raw.is_empty() || raw.len() > CLIENT_IDENTIFIER_MAX_LENGTH {
        return false;
    }
    if raw.eq_ignore_ascii_case(OWN_APP_ID) {
        return false;
    }
    let segments: Vec<&str> = raw.split('.').collect();
    segments.len() >= 2
        && segments.iter().all(|segment| {
            let mut characters = segment.chars();
            characters
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic())
                && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
        })
}

/// A publisher-domain https link (universal link / App Link).
pub fn is_https_link(raw: &str) -> bool {
    raw.len() <= CLIENT_LINK_MAX_LENGTH && is_launchable_url(raw)
}

/// A custom-scheme link whose scheme is not one Takosumi refuses.
pub fn is_custom_scheme_link(raw: &str) -> bool {
    if raw.len() > CLIENT_LINK_MAX_LENGTH
        || raw
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return false;
    }
    let Some((scheme, _)) = raw.split_once(':') else {
        return false;
    };
    let mut characters = scheme.chars();
    let well_formed = (2..=32).contains(&scheme.len())
        && characters
            .next()
            .is_some_and(|first| first.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '+' | '.' | '-')
        });
    well_formed && !BLOCKED_CLIENT_SCHEMES.contains(&scheme)
}

fn is_hex_color(raw: &str) -> bool {
    raw.len() == 7
        && raw.starts_with('#')
        && raw[1..].chars().all(|character| character.is_ascii_hexdigit())
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
            accounts_origin: None,
            scope: None,
            resume: false,
            client: None,
        }
    }

    fn verified_client() -> BannerClient {
        BannerClient {
            name: "Yurucommu".into(),
            verified: true,
            package: Some("com.yurucommu".into()),
            app_link: None,
            universal_link: None,
            open: Some("yurucommu://connect?host_url=https%3A%2F%2Fyc.example".into()),
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

    #[test]
    fn the_shells_own_hosts_never_open() {
        for url in [
            "https://tauri.localhost/",
            "https://ipc.localhost/plugin%3Aapp-screen%7Copen",
            "https://asset.localhost/x",
            "https://TAURI.localhost./",
        ] {
            assert!(!is_launchable_url(url), "{url}");
            assert!(request(url, SHARED_PROFILE).validate().is_err(), "{url}");
        }
    }

    #[test]
    fn the_accounts_origin_scope_and_banner_client_are_rechecked() {
        let mut ok = request("https://app.example/", SHARED_PROFILE);
        ok.accounts_origin = Some("https://accounts.example".into());
        ok.scope = Some("https://takosumi.example|sub_1".into());
        ok.resume = true;
        ok.client = Some(verified_client());
        assert!(ok.validate().is_ok());

        for origin in [
            "https://accounts.example/",
            "https://accounts.example/path",
            "http://accounts.example",
            "https://u@accounts.example",
            "https://tauri.localhost",
            "https://ACCOUNTS.example",
        ] {
            let mut bad = request("https://app.example/", SHARED_PROFILE);
            bad.accounts_origin = Some(origin.into());
            assert!(bad.validate().is_err(), "{origin}");
        }
        let mut bad_scope = request("https://app.example/", SHARED_PROFILE);
        bad_scope.scope = Some("a\nb".into());
        assert!(bad_scope.validate().is_err());

        // An unverified app never carries links into the bar.
        let mut unverified = request("https://app.example/", SHARED_PROFILE);
        unverified.client = Some(BannerClient {
            verified: false,
            ..verified_client()
        });
        assert!(unverified.validate().is_err());
        unverified.client = Some(BannerClient {
            name: "未確認のアプリ".into(),
            verified: false,
            package: None,
            app_link: None,
            universal_link: None,
            open: None,
        });
        assert!(unverified.validate().is_ok());
        let mut spoofed = request("https://app.example/", SHARED_PROFILE);
        spoofed.client = Some(BannerClient {
            name: "Chat\u{202E}exe".into(),
            ..verified_client()
        });
        assert!(spoofed.validate().is_err());
    }

    fn client(platform: &str) -> OpenClientRequest {
        OpenClientRequest {
            platform: platform.into(),
            package: None,
            app_link: None,
            universal_link: None,
            open: None,
        }
    }

    #[test]
    fn native_app_requests_name_a_real_app_and_a_safe_link() {
        let android = OpenClientRequest {
            package: Some("com.yurucommu".into()),
            app_link: Some("https://open.yurucommu.example/connect#host_url=x".into()),
            open: Some("yurucommu://connect?host_url=x".into()),
            ..client("android")
        };
        assert!(android.validate().is_ok());
        let ios = OpenClientRequest {
            universal_link: Some("https://open.yurucommu.example/connect".into()),
            open: Some("yurucommu://connect".into()),
            ..client("ios")
        };
        assert!(ios.validate().is_ok());

        // Android needs its package; iOS has none; one link at least.
        assert!(OpenClientRequest {
            open: Some("yurucommu://x".into()),
            ..client("android")
        }
        .validate()
        .is_err());
        assert!(OpenClientRequest {
            package: Some("com.yurucommu".into()),
            open: Some("yurucommu://x".into()),
            ..client("ios")
        }
        .validate()
        .is_err());
        assert!(OpenClientRequest {
            package: Some("com.yurucommu".into()),
            ..client("android")
        }
        .validate()
        .is_err());
        assert!(client("windows").validate().is_err());

        for package in ["com.takosumi", "COM.TAKOSUMI", "nodots", "1com.x", "com..x", "com.x-y"] {
            let request = OpenClientRequest {
                package: Some(package.into()),
                open: Some("yurucommu://x".into()),
                ..client("android")
            };
            assert!(request.validate().is_err(), "{package}");
        }
        for open in [
            "takosumi://oauth/callback?code=x",
            "intent://x#Intent;scheme=yurucommu;end",
            "javascript:alert(1)",
            "https://x.example/",
            "tel:+81",
            "itms-services://?action=download-manifest",
            "googlechrome://evil.example",
            "Yurucommu://x",
            "yurucommu://x y",
            "y://short-scheme",
            "noscheme",
        ] {
            let request = OpenClientRequest {
                package: Some("com.yurucommu".into()),
                open: Some(open.into()),
                ..client("android")
            };
            assert!(request.validate().is_err(), "{open}");
        }
        for link in ["http://open.example/", "https://u@open.example/", "https://ipc.localhost/"] {
            let request = OpenClientRequest {
                universal_link: Some(link.into()),
                ..client("ios")
            };
            assert!(request.validate().is_err(), "{link}");
        }
    }

    #[test]
    fn the_browser_view_opens_https_pages_only() {
        let view = |url: &str, color: Option<&str>| BrowserViewRequest {
            url: url.into(),
            toolbar_color: color.map(Into::into),
        };
        assert!(view("https://accounts.google.com/", Some("#151517")).validate().is_ok());
        assert!(view("https://accounts.google.com/", None).validate().is_ok());
        assert!(view("http://accounts.google.com/", None).validate().is_err());
        assert!(view("file:///etc/hosts", None).validate().is_err());
        assert!(view("https://tauri.localhost/", None).validate().is_err());
        assert!(view("https://a.example/", Some("red")).validate().is_err());
        assert!(view("https://a.example/", Some("#12345g")).validate().is_err());
    }
}
