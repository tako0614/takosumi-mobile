//! Private parent/child protocol for the opt-in browser-process experiment.
//! These commands are never exposed to remote pages or registered as Tauri IPC.

use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Read, Write};
use std::time::{Duration, Instant};

use crate::{is_isolated_profile, Error, OpenRequest, PermissionDecision, PermissionKind, Result};

pub const MAX_PACKET_BYTES: usize = 64 * 1024;

/// The caller must supply a nonblocking writer. One deadline covers writing
/// and its subsequent reply wait, including partial writes/backpressure.
pub fn write_until(writer: &mut impl Write, mut bytes: &[u8], deadline: Instant) -> io::Result<()> {
    while !bytes.is_empty() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Browser pipe write timed out",
            ));
        }
        match writer.write(bytes) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "Browser pipe closed",
                ))
            }
            Ok(count) => bytes = &bytes[count..],
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub fn read_packet(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    let count = reader
        .take((MAX_PACKET_BYTES + 2) as u64)
        .read_until(b'\n', &mut bytes)?;
    if count == 0 {
        return Ok(None);
    }
    if bytes.pop() != Some(b'\n') || bytes.len() > MAX_PACKET_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid browser pipe frame",
        ));
    }
    Ok(Some(bytes))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "camelCase", deny_unknown_fields)]
pub enum Request {
    Open { id: u64, payload: OpenRequest },
    Close { id: u64, key: String },
    Shutdown { id: u64 },
    /// The App's answer to one permission prompt the browser reported on the
    /// pipe. `prompt_id` names the pending request, never a site or origin:
    /// the browser alone maps it back to the held callback and profile.
    ResolvePermission {
        id: u64,
        prompt_id: u64,
        decision: PermissionDecision,
    },
}

impl Request {
    pub fn id(&self) -> u64 {
        match self {
            Self::Open { id, .. }
            | Self::Close { id, .. }
            | Self::Shutdown { id }
            | Self::ResolvePermission { id, .. } => *id,
        }
    }

    pub fn validate(&self, profile: &str) -> Result<()> {
        if self.id() == 0 || !is_isolated_profile(profile) {
            return Err(Error::InvalidRequest("Invalid browser command identity."));
        }
        match self {
            Self::Open { payload, .. } => {
                payload.validate()?;
                if !payload.web_home || payload.profile != profile || payload.icon_png.is_some() {
                    return Err(Error::InvalidRequest(
                        "Browser process requires its original isolated Web profile.",
                    ));
                }
            }
            Self::Close { key, .. } if key.trim().is_empty() || key.len() > 512 => {
                return Err(Error::InvalidRequest("Invalid browser screen key."));
            }
            Self::ResolvePermission { prompt_id, .. } if *prompt_id == 0 => {
                return Err(Error::InvalidRequest(
                    "Invalid browser permission prompt identity.",
                ));
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "camelCase", deny_unknown_fields)]
pub enum Reply {
    Opened {
        id: u64,
        key: String,
        profile: String,
        browser_id: i32,
        cache_path: String,
        window_id: u64,
        native_pid: u32,
    },
    Closed {
        id: u64,
        key: String,
        profile: String,
        browser_id: i32,
    },
    Stopped {
        id: u64,
    },
    Cancelled {
        id: u64,
        profile: String,
    },
    CloseBlocked {
        id: u64,
        profile: String,
    },
    Error {
        id: u64,
        message: String,
    },
    /// The person chose to open a link outside the App (from its context
    /// menu). Not a reply to any request: the App opens it with the person's
    /// own desktop environment.
    OpenExternal {
        profile: String,
        url: String,
    },
    /// The system asked an owned browser app to quit. Only the parent App
    /// may begin the numbered, sequential shutdown of all its profiles.
    /// This unsolicited intent is never a command reply or a page API.
    QuitRequested {
        profile: String,
    },
    /// The runtime is asking this profile's owner for a site permission
    /// (e.g. notifications, camera). Emitted only when the App opted in to
    /// answering prompts itself: the browser holds the runtime callback open
    /// until the App's `resolvePermission`. Never a command reply; the App
    /// shows it to the person as this profile's own prompt.
    PermissionRequested {
        profile: String,
        prompt_id: u64,
        origin: String,
        kind: PermissionKind,
        permissions: u32,
    },
    /// A held prompt ended on the browser side before any App answer
    /// (dismissed, navigated away or superseded): its `prompt_id` is gone
    /// and must not be answered. Never a command reply.
    PermissionDismissed {
        profile: String,
        prompt_id: u64,
    },
    /// The held callback was resolved with the App's decision.
    PermissionResolved { id: u64 },
}

/// A link the person may open outside the App: an absolute http(s) URL in
/// its ASCII (punycode) form with a host and no credentials, without
/// whitespace, control characters or backslashes, and bounded. Anything else
/// (file:, javascript:, custom schemes) stays inside.
pub fn external_link(url: &str) -> bool {
    link_host(url).is_some()
}

/// The host (with any port) of an `external_link`, shown to the person.
pub fn link_host(url: &str) -> Option<&str> {
    if url.is_empty()
        || url.len() > 4096
        || !url.is_ascii()
        || url.contains('\\')
        || url.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return None;
    }
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // host[:port]: a DNS-style host (letters, digits, dots, hyphens; at most
    // 253) or a bracketed IPv6 address, and a port of at most 65535.
    let (host_ok, port) = match authority.strip_prefix('[') {
        Some(inside) => {
            let (address, after) = inside.split_once(']')?;
            let ok = address.matches(':').count() >= 2
                && address
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.');
            (ok, after)
        }
        None => {
            let (host, port) = authority.split_at(authority.find(':').unwrap_or(authority.len()));
            let ok = !host.is_empty()
                && host.len() <= 253
                && host
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
            (ok, port)
        }
    };
    let port_ok = port.is_empty()
        || port.strip_prefix(':').is_some_and(|digits| {
            !digits.is_empty()
                && digits.chars().all(|c| c.is_ascii_digit())
                && digits.parse::<u32>().is_ok_and(|port| port <= 65535)
        });
    (host_ok && port_ok).then_some(authority)
}

/// Only complete bounded packets cross the pipe; no arbitrary eval or path command.
pub fn decode(bytes: &[u8], profile: &str) -> Result<Request> {
    if bytes.is_empty() || bytes.len() > MAX_PACKET_BYTES || bytes.contains(&b'\n') {
        return Err(Error::InvalidRequest("Invalid browser command packet."));
    }
    let request: Request = serde_json::from_slice(bytes)
        .map_err(|_| Error::InvalidRequest("Invalid browser command JSON."))?;
    request.validate(profile)?;
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILE: &str = "11111111-1111-4111-8111-111111111111";

    fn open() -> serde_json::Value {
        serde_json::json!({"operation":"open","id":1,"payload":{
            "key":format!("web-home:{PROFILE}:https://excalidraw.com"),"url":"https://excalidraw.com/","title":"Work",
            "profile":PROFILE,"webHome":true,"resume":true
        }})
    }

    #[test]
    fn only_bounded_web_links_without_credentials_may_open_outside() {
        for (url, host) in [
            ("https://excalidraw.com/", "excalidraw.com"),
            ("http://example.com/a?b=c#d", "example.com"),
            ("https://www.photopea.com/?x=1", "www.photopea.com"),
            (
                "https://xn--bcher-kva.example:8443/x",
                "xn--bcher-kva.example:8443",
            ),
            ("https://[::1]:8443/", "[::1]:8443"),
            ("https://[::1]/", "[::1]"),
        ] {
            assert!(external_link(url), "{url}");
            assert_eq!(link_host(url), Some(host));
        }
        let long = format!("https://example.com/{}", "a".repeat(4096));
        for url in [
            "",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "takosumi://open",
            "https:///path",
            "https://user:secret@example.com/",
            "https://user@example.com/",
            "https://example.com/a b",
            "https://example.com/\n",
            "https://exa\u{7f}mple.com/",
            "https://exa\u{202e}mple.com/",
            "https://bücher.example/",
            "https://\\evil.com/",
            "https://:443/",
            "https://example.com:x/",
            "https://::1/",
            "https://[]/",
            "https://example.com:/",
            "https://ex[ample.com/",
            "https://a&b.example/",
            "https://a%41.example/",
            "https://user@example.com:443/",
            "https://[evil.com]/",
            "https://[:]x/",
            "https://[:]/",
            "https://example.com:65536/",
            "ftp://example.com/",
            long.as_str(),
        ] {
            assert!(!external_link(url), "{url:?}");
        }
        let reply: Reply = serde_json::from_str(
            r#"{"event":"openExternal","profile":"p","url":"https://example.com/"}"#,
        )
        .unwrap();
        assert!(matches!(reply, Reply::OpenExternal { .. }));
    }

    #[test]
    fn system_quit_intent_carries_no_command_identity() {
        let bytes = serde_json::to_vec(&Reply::QuitRequested {
            profile: PROFILE.into(),
        })
        .unwrap();
        assert!(
            matches!(serde_json::from_slice::<Reply>(&bytes).unwrap(), Reply::QuitRequested { profile } if profile == PROFILE)
        );
        for extra in [r#", "id":0"#, r#", "url":"https://example.com""#] {
            let value = format!(r#"{{"event":"quitRequested","profile":"{PROFILE}"{extra}}}"#);
            assert!(serde_json::from_str::<Reply>(&value).is_err());
        }
    }
    #[test]
    fn shared_open_request_and_its_reuse_flag_survive_the_pipe() {
        let request = decode(&serde_json::to_vec(&open()).unwrap(), PROFILE).unwrap();
        assert!(matches!(request, Request::Open { id: 1, payload } if payload.resume));
    }

    #[test]
    fn permission_requests_events_and_decisions_survive_the_pipe() {
        for (decision, text) in [
            (PermissionDecision::Allow, "allow"),
            (PermissionDecision::Deny, "deny"),
        ] {
            let request = Request::ResolvePermission {
                id: 3,
                prompt_id: 7,
                decision,
            };
            let bytes = serde_json::to_vec(&request).unwrap();
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                value,
                serde_json::json!({"operation":"resolvePermission","id":3,"prompt_id":7,"decision":text})
            );
            assert!(matches!(decode(&bytes, PROFILE).unwrap(), Request::ResolvePermission { id: 3, prompt_id: 7, .. }));
        }
        let missing = br#"{"operation":"resolvePermission","id":3,"prompt_id":0,"decision":"allow"}"#;
        assert!(decode(missing, PROFILE).is_err());
        for reply in [
            serde_json::json!({"event":"permissionRequested","profile":PROFILE,
                "prompt_id":7,"origin":"https://excalidraw.com","kind":"prompt","permissions":8}),
            serde_json::json!({"event":"permissionDismissed","profile":PROFILE,"prompt_id":7}),
            serde_json::json!({"event":"permissionResolved","id":3}),
        ] {
            let reply: Reply = serde_json::from_value(reply).unwrap();
            let bytes = serde_json::to_vec(&reply).unwrap();
            assert!(serde_json::from_slice::<Reply>(&bytes).is_ok());
        }
        let foreign = serde_json::json!({"event":"permissionRequested","profile":"other",
            "prompt_id":7,"origin":"https://excalidraw.com","kind":"media","permissions":3});
        assert!(matches!(
            serde_json::from_value::<Reply>(foreign).unwrap(),
            Reply::PermissionRequested { kind: PermissionKind::Media, permissions: 3, .. }
        ));
        // An extra field or an unknown kind/decision is rejected.
        for bad in [
            serde_json::json!({"event":"permissionRequested","profile":PROFILE,
                "prompt_id":7,"origin":"https://excalidraw.com","kind":"prompt","permissions":8,"extra":1}),
            serde_json::json!({"event":"permissionRequested","profile":PROFILE,
                "prompt_id":7,"origin":"https://excalidraw.com","kind":"other","permissions":8}),
            serde_json::json!({"operation":"resolvePermission","id":3,"prompt_id":7,"decision":"maybe"}),
        ] {
            assert!(serde_json::from_value::<Reply>(bad.clone()).is_err() || decode(&serde_json::to_vec(&bad).unwrap(), PROFILE).is_err());
        }
    }

    #[test]
    fn pipe_rejects_foreign_profiles_shell_urls_and_unbounded_or_unknown_commands() {
        assert!(decode(&serde_json::to_vec(&open()).unwrap(), PROFILE).is_ok());
        for (field, value) in [
            ("profile", serde_json::json!("shared")),
            (
                "profile",
                serde_json::json!("22222222-2222-4222-8222-222222222222"),
            ),
            ("webHome", serde_json::json!(false)),
            ("url", serde_json::json!("https://tauri.localhost/")),
            ("url", serde_json::json!("file:///tmp/app")),
            ("iconPng", serde_json::json!("image")),
        ] {
            let mut packet = open();
            packet["payload"][field] = value;
            assert!(decode(&serde_json::to_vec(&packet).unwrap(), PROFILE).is_err());
        }
        for packet in [
            br#"{"operation":"eval","id":1,"script":"alert(1)"}"#.as_slice(),
            br#"{"operation":"shutdown","id":0}"#.as_slice(),
            br#"{"operation":"shutdown","id":1,"extra":true}"#.as_slice(),
            b"{}\n{}".as_slice(),
        ] {
            assert!(decode(packet, PROFILE).is_err());
        }
        assert!(decode(&vec![b' '; MAX_PACKET_BYTES + 1], PROFILE).is_err());
        let foreign = "22222222-2222-4222-8222-222222222222";
        let mut packet = open();
        packet["payload"]["profile"] = foreign.into();
        packet["payload"]["key"] = format!("web-home:{foreign}:https://excalidraw.com").into();
        let bytes = serde_json::to_vec(&packet).unwrap();
        assert!(decode(&bytes, foreign).is_ok());
        assert!(decode(&bytes, PROFILE).is_err());
    }

    #[test]
    fn reader_bounds_allocation_and_never_accepts_a_partial_packet() {
        let mut reader = io::Cursor::new(b"{}\n{}\n");
        assert_eq!(read_packet(&mut reader).unwrap().unwrap(), b"{}");
        assert_eq!(read_packet(&mut reader).unwrap().unwrap(), b"{}");
        assert!(read_packet(&mut reader).unwrap().is_none());
        assert!(read_packet(&mut io::Cursor::new(b"{}")).is_err());
        assert!(read_packet(&mut io::Cursor::new(vec![b'x'; MAX_PACKET_BYTES + 3])).is_err());
    }
}
