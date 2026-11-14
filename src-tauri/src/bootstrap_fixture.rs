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

// Guest kernel start ticks, not a document clock, host PID or authority proof.
fn process_start_tick(data: &[u8], pid: u32) -> Option<u64> {
    if pid == 0 || pid > i32::MAX as u32 || data.len() > 4096 || !data.ends_with(b"\n") {
        return None;
    }
    if !data.starts_with(format!("{pid} (").as_bytes()) || data.contains(&0) {
        return None;
    }
    let end = data.iter().rposition(|b| *b == b')')?;
    let fields: Vec<_> = data.get(end + 1..)?.strip_prefix(b" ")?
        .split(|b| b.is_ascii_whitespace()).filter(|s| !s.is_empty()).collect();
    if fields.len() < 20 || fields.len() > 64 || matches!(fields[0], b"Z" | b"X") || fields[0].len() != 1 {
        return None;
    }
    let field = fields[19]; // /proc/pid/stat field22, after state (field3).
    if field.is_empty() || field.len() > 19 || field[0] == b'0' || !field.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(field).ok()?.parse::<u64>().ok()
        .filter(|n| *n > 0 && *n <= i64::MAX as u64)
}

fn kernel_boot_id(data: &[u8]) -> Option<String> {
    if data.len() != 37 || data[36] != b'\n' { return None; }
    for (i, byte) in data[..36].iter().enumerate() {
        if matches!(i, 8 | 13 | 18 | 23) {
            if *byte != b'-' { return None; }
        } else if !byte.is_ascii_digit() && !(b'a'..=b'f').contains(byte) { return None; }
    }
    Some(data[..36].iter().filter(|b| **b != b'-').map(|b| *b as char).collect())
}

fn android_packet(pid: u32, start_tick: u64, boot: &str, line: &str) -> String {
    format!("v=1 phase=ROOT_MAIN pid={pid} startTick={start_tick} boot={boot} {line} rootEnd=1")
}

#[cfg(target_os = "android")]
pub fn install_android<R: tauri::Runtime>(app: &tauri::App<R>) {
    use std::io::Read;
    use std::sync::{Arc, Mutex};
    use tauri::Listener;

    if !cfg!(debug_assertions) || app.config().identifier != "com.takosumi.fixture.android" {
        return;
    }
    let pid = std::process::id();
    let mut stat = Vec::new();
    let Some(start_tick) = std::fs::File::open("/proc/self/stat").ok()
        .and_then(|file| file.take(4097).read_to_end(&mut stat).ok())
        .and_then(|_| process_start_tick(&stat, pid)) else { return };
    let mut boot = Vec::new();
    let Some(boot) = std::fs::File::open("/proc/sys/kernel/random/boot_id").ok()
        .and_then(|file| file.take(38).read_to_end(&mut boot).ok())
        .and_then(|_| kernel_boot_id(&boot)) else { return };
    let recorder = Arc::new(Mutex::new(Recorder::new(true)));
    app.listen("takosumi-fixture-bootstrap", move |event| {
        let line = recorder.lock().ok().and_then(|mut r| r.record(event.payload()));
        if let Some(line) = line {
            let packet = android_packet(pid, start_tick, &boot, &line);
            if let Ok(message) = std::ffi::CString::new(packet) {
                // Private diagnostic only. Log loss/blocking must not alter the
                // Root event or UI verdict; no new invoke command/permission.
                #[link(name = "log")]
                unsafe extern "C" {
                    fn __android_log_write(priority: i32, tag: *const std::ffi::c_char,
                                           text: *const std::ffi::c_char) -> i32;
                }
                unsafe { __android_log_write(4, c"TakosRootBootstrap".as_ptr(), message.as_ptr()); }
            }
        }
    });
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
    fn android_birth_and_envelope_reuse_the_closed_formatter() {
        let mut fields = vec!["S", "1"];
        fields.extend(["0"; 17]); fields.push("1234"); fields.extend(["0"; 8]);
        let data = format!("42 (fixture (root)) {}\n", fields.join(" "));
        assert_eq!(process_start_tick(data.as_bytes(), 42), Some(1234));
        assert_eq!(process_start_tick(data.as_bytes(), 43), None);
        assert_eq!(process_start_tick(data.trim_end().as_bytes(), 42), None);
        assert_eq!(process_start_tick(&vec![b'x'; 4097], 42), None);
        for bad in [data.replace("1234", "0"), data.replace("1234", "SECRET"),
                    data.replace("1234", "01234"), data.replace("1234", "9223372036854775808"),
                    data.replace(") S ", ") Z ")] {
            assert_eq!(process_start_tick(bad.as_bytes(), 42), None);
        }
        let raw_boot = b"01234567-89ab-cdef-0123-456789abcdef\n";
        let boot = kernel_boot_id(raw_boot).unwrap();
        assert_eq!(boot, "0123456789abcdef0123456789abcdef");
        assert_eq!(kernel_boot_id(&raw_boot[..36]), None);
        assert_eq!(kernel_boot_id(b"SECRET"), None);
        for (i, stage) in STAGES.iter().enumerate() {
            let line = Recorder::new(true).record(&format!("[1,1,\"{stage}\",600000]")).unwrap();
            let packet = android_packet(42, 1234, &boot, &line);
            assert!(packet.ends_with(" end=1 rootEnd=1"));
            assert!(packet.len() < 320);
            println!("{packet}");
            assert!(i < 16);
        }
        let unknown = Recorder::new(true).record("[1,1,\"SECRET\",0]").unwrap();
        assert!(!unknown.contains("SECRET"));
        println!("{}", android_packet(42, 1234, &boot, &unknown));
        let mut recorder = Recorder::new(true);
        let cap = (0..33).filter_map(|_| recorder.record("[1,1,\"entry\",null]")).last().unwrap();
        assert!(cap.contains("stage=cap"));
        println!("{}", android_packet(42, 1234, &boot, &cap));
    }

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
