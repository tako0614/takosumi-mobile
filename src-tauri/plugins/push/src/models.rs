use serde::{Deserialize, Serialize};

use crate::Error;

const SUBSCRIPTION_ID_MIN: usize = 8;
const SUBSCRIPTION_ID_MAX: usize = 128;
const ORIGIN_MAX_LENGTH: usize = 256;
const ACCOUNT_MAX_CHARS: usize = 512;
const LABEL_MAX_CHARS: usize = 128;
/// The instance origin and its Accounts (issuer) origin; a little headroom.
const ORIGINS_MAX: usize = 4;

/// One Takosumi instance subscription's keys: the relay subscription id the
/// relay puts in `s`, the instance and account it belongs to, the name its
/// notification channel shows, and the origins a notification may open.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateKeysRequest {
    pub subscription_id: String,
    pub instance: String,
    #[serde(default)]
    pub account: String,
    pub label: String,
    pub origins: Vec<String>,
}

impl CreateKeysRequest {
    pub fn validate(&self) -> crate::Result<()> {
        validate_subscription_id(&self.subscription_id)?;
        if !is_web_origin(&self.instance) {
            return Err(Error::InvalidRequest("instance must be an https origin"));
        }
        if self.account.chars().count() > ACCOUNT_MAX_CHARS || has_control(&self.account) {
            return Err(Error::InvalidRequest("account is invalid"));
        }
        let label = self.label.trim();
        if label.is_empty() || label.chars().count() > LABEL_MAX_CHARS || has_control(label) {
            return Err(Error::InvalidRequest("label must be 1-128 printable characters"));
        }
        if self.origins.is_empty() || self.origins.len() > ORIGINS_MAX {
            return Err(Error::InvalidRequest("origins must name 1-4 origins"));
        }
        if self.origins.iter().any(|origin| !is_web_origin(origin)) {
            return Err(Error::InvalidRequest("origins must be https origins"));
        }
        if !self.origins.iter().any(|origin| origin == &self.instance) {
            return Err(Error::InvalidRequest("origins must include the instance"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionRequest {
    pub subscription_id: String,
}

impl SubscriptionRequest {
    pub fn validate(&self) -> crate::Result<()> {
        validate_subscription_id(&self.subscription_id)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionStateResponse {
    /// `granted`, `denied` or `prompt`.
    pub state: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionResponse {
    pub granted: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenResponse {
    pub token: String,
    /// `fcm` or `apns`.
    pub provider: String,
    /// `production`, or `development` for the APNs sandbox.
    pub environment: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceNameResponse {
    pub name: String,
}

/// The subscription's public half, for the instance's pusher registration:
/// `p256dh` (uncompressed P-256 point) and the 16-byte `auth`, both unpadded
/// base64url. The private key never leaves the native side.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeysResponse {
    pub p256dh: String,
    pub auth: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionSummary {
    pub subscription_id: String,
    pub instance: String,
    #[serde(default)]
    pub account: String,
    pub label: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionList {
    pub subscriptions: Vec<SubscriptionSummary>,
}

/// A tapped notification: which instance and account to switch to and, when
/// the message could be read, the page to open (on one of its origins).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationTap {
    pub id: String,
    pub subscription_id: String,
    pub instance: String,
    #[serde(default)]
    pub account: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub navigate: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingTapResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tap: Option<NotificationTap>,
}

fn validate_subscription_id(value: &str) -> crate::Result<()> {
    let valid = (SUBSCRIPTION_ID_MIN..=SUBSCRIPTION_ID_MAX).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidRequest("subscriptionId is invalid"))
    }
}

fn has_control(value: &str) -> bool {
    value.chars().any(char::is_control)
}

/// `https://host[:port]` (lowercase, nothing after the authority), or plain
/// http on a loopback host for a development instance.
pub fn is_web_origin(value: &str) -> bool {
    if value.len() > ORIGIN_MAX_LENGTH || value != value.to_ascii_lowercase() {
        return false;
    }
    let authority = if let Some(rest) = value.strip_prefix("https://") {
        rest
    } else if let Some(rest) = value.strip_prefix("http://") {
        let host = rest.rsplit_once(':').map_or(rest, |(host, _)| host);
        if !matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
            return false;
        }
        rest
    } else {
        return false;
    };
    !authority.is_empty()
        && !authority.starts_with(':')
        && authority.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':' | b'[' | b']')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> CreateKeysRequest {
        CreateKeysRequest {
            subscription_id: "AbCdEfGhIjKlMnOpQrStUv".into(),
            instance: "https://takosumi.example".into(),
            account: "acct_1".into(),
            label: "takosumi.example".into(),
            origins: vec![
                "https://takosumi.example".into(),
                "https://accounts.takosumi.example".into(),
            ],
        }
    }

    #[test]
    fn accepts_a_relay_subscription_for_an_instance() {
        request().validate().expect("valid");
        SubscriptionRequest {
            subscription_id: "AbCdEfGhIjKlMnOpQrStUv".into(),
        }
        .validate()
        .expect("valid id");
    }

    #[test]
    fn refuses_bad_ids_origins_and_labels() {
        for id in ["short", "has space here!", "slash/inside/the/id", &"a".repeat(129)] {
            let mut bad = request();
            bad.subscription_id = id.into();
            assert!(bad.validate().is_err(), "{id}");
        }
        for origin in [
            "http://takosumi.example",
            "https://takosumi.example/",
            "https://Takosumi.example",
            "https://user@takosumi.example",
            "https://takosumi.example/path",
            "takosumi://app",
            "https://",
        ] {
            let mut bad = request();
            bad.instance = origin.into();
            bad.origins = vec![origin.into()];
            assert!(bad.validate().is_err(), "{origin}");
        }
        let mut missing_instance = request();
        missing_instance.origins = vec!["https://accounts.takosumi.example".into()];
        assert!(missing_instance.validate().is_err());
        let mut too_many = request();
        too_many.origins = (0..5).map(|n| format!("https://h{n}.example")).collect();
        too_many.origins.push(too_many.instance.clone());
        assert!(too_many.validate().is_err());
        for label in ["", "   ", "tab\there", &"x".repeat(129)] {
            let mut bad = request();
            bad.label = label.into();
            assert!(bad.validate().is_err(), "{label:?}");
        }
    }

    #[test]
    fn loopback_http_is_a_development_origin() {
        assert!(is_web_origin("http://localhost:8787"));
        assert!(is_web_origin("http://127.0.0.1"));
        assert!(is_web_origin("http://[::1]:3000"));
        assert!(!is_web_origin("http://localhost.example"));
        assert!(!is_web_origin("http://10.0.0.2"));
    }

    #[test]
    fn a_tap_without_a_page_omits_navigate() {
        let tap = NotificationTap {
            id: "t1".into(),
            subscription_id: "AbCdEfGhIjKlMnOpQrStUv".into(),
            instance: "https://takosumi.example".into(),
            account: String::new(),
            navigate: None,
        };
        let json = serde_json::to_value(&tap).expect("serializes");
        assert!(json.get("navigate").is_none());
        assert_eq!(json["subscriptionId"], "AbCdEfGhIjKlMnOpQrStUv");
    }
}
