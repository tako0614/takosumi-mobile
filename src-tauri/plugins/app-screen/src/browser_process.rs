//! Private parent/child protocol for the opt-in browser-process experiment.
//! These commands are never exposed to remote pages or registered as Tauri IPC.

use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Read, Write};
use std::time::{Duration, Instant};

use crate::{is_isolated_profile, Error, OpenRequest, Result};

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
}

impl Request {
    pub fn id(&self) -> u64 {
        match self {
            Self::Open { id, .. } | Self::Close { id, .. } | Self::Shutdown { id } => *id,
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
    fn shared_open_request_and_its_reuse_flag_survive_the_pipe() {
        let request = decode(&serde_json::to_vec(&open()).unwrap(), PROFILE).unwrap();
        assert!(matches!(request, Request::Open { id: 1, payload } if payload.resume));
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
