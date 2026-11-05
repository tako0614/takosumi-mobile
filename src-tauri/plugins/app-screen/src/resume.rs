//! Last primary Web page, scoped by the validated connection and native profile.
//! This is a convenience preference, never a credential or crash-recovery store.

const MAX_URL_BYTES: usize = 4_096;
const AUTH_PATHS: &[&str] = &[
    "/oauth/authorize",
    "/oauth/upstream/authorize",
    "/oauth/upstream/callback",
    "/sign-in",
    "/sign-in/callback",
];
const CREDENTIAL_NAMES: &[&str] = &[
    "accesskey",
    "accesstoken",
    "apikey",
    "auth",
    "authorization",
    "bearertoken",
    "clientsecret",
    "cookie",
    "credential",
    "credentials",
    "credentialvalue",
    "jwt",
    "passwd",
    "password",
    "privatekey",
    "proxyauthorization",
    "refreshtoken",
    "secret",
    "session",
    "sessionid",
    "sessiontoken",
    "sig",
    "signature",
    "signingkey",
    "token",
    "xamzcredential",
    "xamzsecuritytoken",
    "xamzsignature",
    "xgoogcredential",
    "xgoogsignature",
    "xapikey",
    "code",
    "idtoken",
    "oauthtoken",
    "oauthverifier",
    "samlresponse",
    "ticket",
];

pub(crate) fn rememberable(raw: &str) -> bool {
    if raw.len() > MAX_URL_BYTES
        || raw
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '#' | '\\'))
        || !crate::models::is_launchable_url(raw)
    {
        return false;
    }
    if tauri::Url::parse(raw).is_ok_and(|url| AUTH_PATHS.contains(&url.path())) {
        return false;
    }
    let query = raw.split_once('?').map_or("", |(_, query)| query);
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .all(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            let (Some(name), Some(value)) = (decode(name), decode(value)) else {
                return false;
            };
            let name: String = name
                .to_lowercase()
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect();
            value.trim().is_empty() || !CREDENTIAL_NAMES.contains(&name.as_str())
        })
}

fn decode(part: &str) -> Option<String> {
    let mut decoded = Vec::with_capacity(part.len());
    let bytes = part.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        decoded.push(match bytes[index] {
            b'%' => {
                let high = (*bytes.get(index + 1)? as char).to_digit(16)?;
                let low = (*bytes.get(index + 2)? as char).to_digit(16)?;
                index += 2;
                ((high << 4) | low) as u8
            }
            b'+' => b' ',
            byte => byte,
        });
        index += 1;
    }
    String::from_utf8(decoded).ok()
}

#[cfg(feature = "desktop")]
mod store {
    use super::rememberable;
    use serde::{Deserialize, Serialize};
    use std::{
        fs::{File, OpenOptions},
        io::{self, Read, Write},
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    const MAX_PAGES: usize = 64;
    const MAX_FILE_BYTES: u64 = 512 * 1024;
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    #[derive(Serialize, Deserialize)]
    struct Page {
        key: String,
        url: String,
    }

    #[derive(Serialize, Deserialize)]
    struct Document {
        version: u8,
        pages: Vec<Page>,
    }

    #[derive(Clone)]
    pub(crate) struct ResumeStore {
        path: PathBuf,
        key: String,
    }

    impl ResumeStore {
        pub(crate) fn new(directory: PathBuf, key: String) -> Self {
            Self {
                path: directory.join("takosumi-last-pages.v1.json"),
                key,
            }
        }

        fn read(&self) -> io::Result<Vec<Page>> {
            let file = match File::open(&self.path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(error) => return Err(error),
            };
            let mut bytes = Vec::new();
            file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_FILE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Last-page preference is too large",
                ));
            }
            // Parser diagnostics can include input values. Keep private URLs
            // out of logs even when this preference is malformed.
            let document: Document = serde_json::from_slice(&bytes).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid last-page preference JSON",
                )
            })?;
            if document.version != 1
                || document.pages.len() > MAX_PAGES
                || document.pages.iter().any(|page| {
                    page.key.is_empty() || page.key.len() > 512 || !rememberable(&page.url)
                })
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid last-page preference",
                ));
            }
            Ok(document.pages)
        }

        pub(crate) fn recall(&self) -> io::Result<Option<String>> {
            Ok(self
                .read()?
                .into_iter()
                .rev()
                .find(|page| page.key == self.key)
                .map(|page| page.url))
        }

        pub(crate) fn record(&self, url: &str) -> io::Result<()> {
            if !rememberable(url) {
                return Ok(());
            }
            let mut pages = self.read()?;
            if pages
                .last()
                .is_some_and(|page| page.key == self.key && page.url == url)
            {
                return Ok(());
            }
            pages.retain(|page| page.key != self.key);
            pages.push(Page {
                key: self.key.clone(),
                url: url.into(),
            });
            if pages.len() > MAX_PAGES {
                pages.remove(0);
            }
            let bytes =
                serde_json::to_vec(&Document { version: 1, pages }).map_err(io::Error::other)?;
            let temporary = self.path.with_extension(format!(
                "tmp-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            // Remove only a temporary file created by this call. Failed
            // create_new never deletes somebody else's existing file.
            let mut file = options.open(&temporary)?;
            let written = file.write_all(&bytes);
            drop(file);
            let result = written.and_then(|_| std::fs::rename(&temporary, &self.path));
            if result.is_err() {
                let _ = std::fs::remove_file(&temporary);
            }
            result
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        struct Directory(PathBuf);
        impl Directory {
            fn new() -> Self {
                let path = std::env::temp_dir().join(format!(
                    "sumi-resume-{}-{}",
                    std::process::id(),
                    NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::create_dir(&path).unwrap();
                Self(path)
            }
        }
        impl Drop for Directory {
            fn drop(&mut self) {
                for entry in std::fs::read_dir(&self.0).unwrap() {
                    std::fs::remove_file(entry.unwrap().path()).unwrap();
                }
                std::fs::remove_dir(&self.0).unwrap();
            }
        }

        #[test]
        fn new_instances_restore_only_their_connection_and_profile() {
            let work_dir = Directory::new();
            let personal_dir = Directory::new();
            let work = ResumeStore::new(
                work_dir.0.clone(),
                "web-home:work:https://sumi.example".into(),
            );
            let personal = ResumeStore::new(personal_dir.0.clone(), work.key.clone());
            work.record("https://office.example/document/work?view=edit")
                .unwrap();
            personal
                .record("https://office.example/document/personal")
                .unwrap();
            let restored = ResumeStore::new(work_dir.0.clone(), work.key.clone());
            assert_eq!(
                restored.recall().unwrap().as_deref(),
                Some("https://office.example/document/work?view=edit")
            );
            assert_eq!(
                personal.recall().unwrap().as_deref(),
                Some("https://office.example/document/personal")
            );
            let other = ResumeStore::new(
                work_dir.0.clone(),
                "web-home:other:https://sumi.example".into(),
            );
            assert_eq!(other.recall().unwrap(), None);
            work.record("https://accounts.example/callback?code=fixture-secret")
                .unwrap();
            assert_eq!(restored.recall().unwrap(), work.recall().unwrap());
            assert!(!std::fs::read_to_string(work.path)
                .unwrap()
                .contains("fixture-secret"));
        }

        #[test]
        fn unknown_or_corrupt_preferences_are_not_overwritten() {
            let directory = Directory::new();
            let store = ResumeStore::new(
                directory.0.clone(),
                "web-home:work:https://sumi.example".into(),
            );
            for bytes in ["{", r#"{"version":2,"pages":[]}"#] {
                std::fs::write(&store.path, bytes).unwrap();
                assert!(store.recall().is_err());
                assert!(store.record("https://office.example/document/1").is_err());
                assert_eq!(std::fs::read_to_string(&store.path).unwrap(), bytes);
            }
        }
    }
}

#[cfg(feature = "desktop")]
pub(crate) use store::ResumeStore;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_shared_web_resume_policy_and_credential_names() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../web-home-resume-cases.json")).unwrap();
        for case in cases["cases"].as_array().unwrap() {
            let url = case["url"].as_str().unwrap();
            assert_eq!(
                rememberable(url),
                case["remember"].as_bool().unwrap(),
                "{url}"
            );
        }
        let rules: serde_json::Value =
            serde_json::from_str(include_str!("../resume-url-cases.json")).unwrap();
        assert_eq!(
            CREDENTIAL_NAMES,
            rules["credentialQueryNames"]
                .as_array()
                .unwrap()
                .iter()
                .map(|name| name.as_str().unwrap())
                .collect::<Vec<_>>()
        );
        let navigation: serde_json::Value =
            serde_json::from_str(include_str!("../navigation-policy-cases.json")).unwrap();
        assert_eq!(
            AUTH_PATHS,
            navigation["accountsSignInPaths"]
                .as_array()
                .unwrap()
                .iter()
                .map(|path| path.as_str().unwrap())
                .collect::<Vec<_>>()
        );
        assert!(!rememberable(&format!(
            "https://office.example/{}",
            "a".repeat(MAX_URL_BYTES)
        )));
    }
}
