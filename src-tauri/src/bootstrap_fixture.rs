//! Private root-bootstrap hints on the existing event/stdout log channels.
//! Receipt order/time is not browser execution time, paint, AX or authority.

const PREFIX: &str = "com.takosumi.fixture.simulator.bootstrap";
const STAGES: &[&str] = &[
    "entry",
    "storage_begin",
    "storage_end",
    "storage_unavailable",
    "selection_begin",
    "selection_end",
    "legacy",
    "home_init",
    "connections_begin",
    "connections_end",
    "connections_failed",
    "save_element",
    "render_returned",
];

fn number(value: &str, max: u32) -> Option<u32> {
    let value = value.trim_matches([' ', '\t', '\r', '\n']);
    if value.is_empty()
        || !value.bytes().all(|b| b.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return None;
    }
    value.parse::<u32>().ok().filter(|v| *v <= max)
}

/// Deliberately accepts only the fixed ASCII JSON tuple produced by the fixture.
/// JSON objects, escaped/arbitrary strings and extra fields become unknown.
fn hint(payload: &str) -> (u32, &'static str, Option<u32>) {
    let unknown = (0, "unknown", None);
    if payload.len() > 128
        || !payload.is_ascii()
        || payload
            .bytes()
            .any(|b| b < 32 && !matches!(b, b'\t' | b'\r' | b'\n'))
    {
        return unknown;
    }
    let text = payload.trim_matches([' ', '\t', '\r', '\n']);
    let Some(body) = text.strip_prefix('[').and_then(|v| v.strip_suffix(']')) else {
        return unknown;
    };
    let parts: Vec<_> = body.split(',').map(str::trim).collect();
    if parts.len() != 4 || parts[0] != "1" {
        return unknown;
    }
    let Some(sequence) = number(parts[1], 16) else {
        return unknown;
    };
    let stage = parts[2].strip_prefix('"').and_then(|v| v.strip_suffix('"'));
    if stage == Some("cap") && sequence == 0 && parts[3] == "null" {
        return (0, "cap", None);
    }
    let Some(stage) = STAGES.iter().copied().find(|v| Some(*v) == stage) else {
        return unknown;
    };
    if sequence == 0 {
        return unknown;
    }
    let elapsed = if parts[3] == "null" {
        None
    } else {
        let Some(value) = number(parts[3], 600_000) else {
            return unknown;
        };
        Some(value)
    };
    (sequence, stage, elapsed)
}

struct Recorder {
    enabled: bool,
    emitted: u32,
}

impl Recorder {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            emitted: 0,
        }
    }

    fn record(&mut self, payload: &str) -> Option<String> {
        if !self.enabled || self.emitted > 32 {
            return None;
        }
        self.emitted += 1;
        let (sequence, stage, elapsed) = if self.emitted == 33 {
            (0, "cap", None)
        } else {
            hint(payload)
        };
        Some(format!(
            "{PREFIX} v=1 receipt={} seq={sequence} stage={stage} elapsedMs={} end=1",
            self.emitted,
            elapsed.map(i64::from).unwrap_or(-1)
        ))
    }
}

#[cfg(target_os = "ios")]
pub fn install<R: tauri::Runtime>(app: &tauri::App<R>) {
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tauri::Listener;

    if !cfg!(debug_assertions) || app.config().identifier != "com.takosumi.fixture.simulator" {
        return;
    }
    let recorder = Arc::new(Mutex::new(Recorder::new(true)));
    app.listen("takosumi-fixture-bootstrap", move |event| {
        let line = recorder
            .lock()
            .ok()
            .and_then(|mut r| r.record(event.payload()));
        if let Some(line) = line {
            // The mobile entry macro already redirects stderr to Tauri's iOS
            // Logger. A write can block or be lost; an IO error is diagnostic only.
            let _ = writeln!(std::io::stderr().lock(), "{line}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_payloads_and_browser_clock_do_not_serialize_private_input() {
        let mut recorder = Recorder::new(true);
        for (i, stage) in STAGES.iter().enumerate() {
            let line = recorder
                .record(&format!("[1,{},\"{stage}\",{i}]", i + 1))
                .unwrap();
            assert!(line.ends_with(&format!("seq={} stage={stage} elapsedMs={i} end=1", i + 1)));
            println!("{line}");
        }
        let wide = recorder.record("[1,14,\"entry\",600000]").unwrap();
        assert!(wide.contains("elapsedMs=600000"));
        println!("{wide}");
        for payload in [
            "[1,1,\"SECRET\",0]",
            "[1,1,\"entry\",\"SECRET\"]",
            "[1,1,\"entry\",0,\"SECRET\"]",
            "{\"SECRET\":true}",
            "[1,true,\"entry\",0]",
            "[2,1,\"entry\",0]",
            "[1,0,\"entry\",0]",
            "[1,17,\"entry\",0]",
            "[1,1,\"entry\",-1]",
            "[1,1,\"entry\",600001]",
            "[1,1,\"entry\",1.0]",
            "[1,1,\"entry\",NaN]",
            "[1,01,\"entry\",0]",
            "[1,1,\"\\u0065ntry\",0]",
            "[1,1,\"entry\",0]\nSECRET",
        ] {
            let line = Recorder::new(true).record(payload).unwrap();
            assert!(line.ends_with("seq=0 stage=unknown elapsedMs=-1 end=1"));
            assert!(!line.contains("SECRET"));
        }
        assert_eq!(hint(&"x".repeat(129)), (0, "unknown", None));
        assert_eq!(hint("[1,1,\"entry\",null]"), (1, "entry", None));
        assert_eq!(hint("[1,1,\"entry\",600000]"), (1, "entry", Some(600_000)));
        assert_eq!(hint("[1,0,\"cap\",null]"), (0, "cap", None));
    }

    #[test]
    fn disabled_gate_and_one_cap_keep_the_log_bound() {
        assert!(Recorder::new(false).record("SECRET").is_none());
        let mut recorder = Recorder::new(true);
        let lines: Vec<_> = (0..40)
            .filter_map(|_| recorder.record("[1,16,\"render_returned\",600000]"))
            .collect();
        assert_eq!(lines.len(), 33);
        assert!(lines[32].ends_with("receipt=33 seq=0 stage=cap elapsedMs=-1 end=1"));
        assert!(lines.iter().all(|line| line.len() < 128));
        assert!(!lines.join("\n").contains("ready"));
    }
}
