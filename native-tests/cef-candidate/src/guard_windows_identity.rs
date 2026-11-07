//! Diagnostics only: who the processes the Windows guard's drain forced were.
//!
//! When the drain reaches `TerminateJobObject`, the guard reads, for each
//! process still in the job, its image file name (never the path) and its
//! Chromium process type (`--type=` and `--utility-sub-type=` values only,
//! `process_type.rs`), while the process still runs: a terminated process's
//! command line is gone with its address space. After the count it writes
//! one line to its standard error, the browser's log (`browser.log`):
//!
//! `takosumi-browser-guard: forced-cleanup {"guardPid":G,"forced":N,
//! "processes":[{"pid","image","type","utilitySubType"?,"exit"}...],
//! "enteredAfterList":M,"endedByThemselves":K,"identifyMs":T}`
//!
//! for the processes it counted as forced, with each one's exit code after
//! the termination (`0xE0007462`, or `running`) as the count classified it
//! (`classify`), how many listed processes ended by themselves (any other
//! exit code), and how long the identification took (`identifyMs`, which
//! lengthens the natural window). It is written whenever the drain reaches
//! the termination, so with no forced process too; `guardPid` names this
//! guard, since the log is the profile's and later launches append to it.
//! A failed read is
//! `"type":"unknown"` (or `"image":"unknown"`) with the error's name. The
//! receipt is unchanged, so it keeps the Linux guard's contract.

use super::process_type::{process_type, windows_arguments, ProcessType};
use std::ffi::{c_void, OsString};
use std::io::{self, Write};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, OwnedHandle};

/// The start of the diagnostic line (windows_browser_process.py collects it).
pub(super) const FORCED_DIAGNOSTIC: &str = "takosumi-browser-guard: forced-cleanup ";

#[link(name = "kernel32")]
extern "system" {
    fn QueryFullProcessImageNameW(
        process: *mut c_void,
        flags: u32,
        name: *mut u16,
        size: *mut u32,
    ) -> i32;
}

#[link(name = "ntdll")]
extern "system" {
    fn NtQueryInformationProcess(
        process: *mut c_void,
        class: u32,
        information: *mut c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}

/// `ProcessCommandLineInformation` (Windows 8.1 and later): a UNICODE_STRING
/// followed by its text, for a handle with PROCESS_QUERY_LIMITED_INFORMATION.
const PROCESS_COMMAND_LINE_INFORMATION: u32 = 60;
const STATUS_BUFFER_OVERFLOW: u32 = 0x8000_0005;
const STATUS_INFO_LENGTH_MISMATCH: u32 = 0xC000_0004;
const STATUS_BUFFER_TOO_SMALL: u32 = 0xC000_0023;
/// The longest command line Windows allows, as UTF-16, plus the header.
const MAX_COMMAND_LINE: u32 = 32_768 * 2 + 64;

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *const u16,
}

/// Who a listed process is, read before the termination.
pub(super) struct Identity {
    image: Result<String, String>,
    kind: Result<ProcessType, String>,
}

pub(super) fn identify(handle: &io::Result<OwnedHandle>) -> Identity {
    match handle {
        Ok(handle) => Identity {
            image: image_name(handle),
            kind: command_line(handle)
                .map(|line| process_type(windows_arguments(&line).iter().map(String::as_str))),
        },
        Err(error) => Identity {
            image: Err(super::fault(error)),
            kind: Err(super::fault(error)),
        },
    }
}

/// The executable's file name, without its directory.
fn image_name(handle: &OwnedHandle) -> Result<String, String> {
    let mut buffer = vec![0u16; 32_768];
    let mut size = buffer.len() as u32;
    // SAFETY: a live process handle and a buffer of `size` UTF-16 units.
    let queried = unsafe {
        QueryFullProcessImageNameW(
            handle.as_raw_handle().cast(),
            0,
            buffer.as_mut_ptr(),
            &mut size,
        )
    };
    if queried == 0 {
        return Err(super::fault(&io::Error::last_os_error()));
    }
    let path = OsString::from_wide(&buffer[..size as usize])
        .to_string_lossy()
        .into_owned();
    Ok(path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or_default()
        .to_owned())
}

/// The process's command line. It stays in this module: only its process
/// type tokens leave it.
fn command_line(handle: &OwnedHandle) -> Result<String, String> {
    let mut length = 4096u32;
    for _ in 0..3 {
        // u64 units keep the UNICODE_STRING header aligned.
        let mut buffer = vec![0u64; (length as usize).div_ceil(8)];
        let bytes = (buffer.len() * 8) as u32;
        let mut returned = 0u32;
        // SAFETY: a live process handle and a writable buffer of `bytes`.
        let status = unsafe {
            NtQueryInformationProcess(
                handle.as_raw_handle().cast(),
                PROCESS_COMMAND_LINE_INFORMATION,
                buffer.as_mut_ptr().cast(),
                bytes,
                &mut returned,
            )
        } as u32;
        if matches!(
            status,
            STATUS_BUFFER_OVERFLOW | STATUS_INFO_LENGTH_MISMATCH | STATUS_BUFFER_TOO_SMALL
        ) && returned > bytes
            && returned <= MAX_COMMAND_LINE
        {
            length = returned;
            continue;
        }
        if status & 0x8000_0000 != 0 {
            return Err(format!("NTSTATUS 0x{status:08X}"));
        }
        if (returned as usize) < std::mem::size_of::<UnicodeString>() {
            return Err("CommandLineTooShort".into());
        }
        // SAFETY: the call succeeded and wrote at least a header at the
        // buffer's (8-aligned) start.
        let header = unsafe { &*buffer.as_ptr().cast::<UnicodeString>() };
        let start = buffer.as_ptr() as usize + std::mem::size_of::<UnicodeString>();
        let end = buffer.as_ptr() as usize + bytes as usize;
        let text = header.buffer as usize;
        let size = header.length as usize;
        if size == 0 {
            return Ok(String::new());
        }
        if header.buffer.is_null()
            || !size.is_multiple_of(2)
            || !text.is_multiple_of(2)
            || text < start
            || text + size > end
        {
            return Err("CommandLineOutsideBuffer".into());
        }
        // SAFETY: the text lies within the buffer, as checked above.
        let units = unsafe { std::slice::from_raw_parts(header.buffer, size / 2) };
        return Ok(String::from_utf16_lossy(units));
    }
    Err("CommandLineKeptGrowing".into())
}

/// One forced process of the diagnostic line.
pub(super) fn entry(pid: u32, identity: &Identity, exit: Option<u32>) -> serde_json::Value {
    let mut entry = serde_json::json!({
        "pid": pid,
        "exit": exit.map_or_else(|| "running".to_owned(), |code| format!("0x{code:08X}")),
    });
    match &identity.image {
        Ok(name) => entry["image"] = name.as_str().into(),
        Err(error) => {
            entry["image"] = "unknown".into();
            entry["imageError"] = error.as_str().into();
        }
    }
    match &identity.kind {
        Ok(found) => {
            entry["type"] = found.kind.as_deref().unwrap_or("none").into();
            if let Some(sub) = &found.utility_sub_type {
                entry["utilitySubType"] = sub.as_str().into();
            }
        }
        Err(error) => {
            entry["type"] = "unknown".into();
            entry["error"] = error.as_str().into();
        }
    }
    entry
}

/// Write the diagnostic line, in one write, to the guard's standard error.
pub(super) fn report(
    forced: u32,
    processes: Vec<serde_json::Value>,
    entered_after_list: u32,
    ended_by_themselves: u32,
    identify_ms: u128,
) {
    let line = format!(
        "{FORCED_DIAGNOSTIC}{}\n",
        serde_json::json!({
            "guardPid": std::process::id(),
            "forced": forced,
            "processes": processes,
            "enteredAfterList": entered_after_list,
            "endedByThemselves": ended_by_themselves,
            "identifyMs": identify_ms as u64,
        })
    );
    let _ = io::stderr().lock().write_all(line.as_bytes());
}
