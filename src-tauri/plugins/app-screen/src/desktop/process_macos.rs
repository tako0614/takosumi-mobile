//! macOS pieces of the per-profile browser adapter (`process_adapter.rs`,
//! M2): the identity checks on a browser's `opened` reply, Chromium's profile
//! lock and the external-link opener. The rest is POSIX and shared with
//! Linux. libproc (`proc_pidinfo`, `proc_pidpath`) through libc.
use std::path::Path;

/// What the kernel says about one process: its parent and start time
/// (microseconds since the epoch); `None` when no such process exists.
pub(super) fn parent_and_start(pid: u32) -> Option<(u32, u64)> {
    // SAFETY: proc_bsdinfo is plain data; the call writes at most its size.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let read = unsafe {
        libc::proc_pidinfo(
            i32::try_from(pid).ok()?,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    (read == size && info.pbi_status != libc::SZOMB).then(|| {
        (
            info.pbi_ppid,
            info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
        )
    })
}

/// A running process's executable path (`proc_pidpath`).
pub(super) fn image_of(pid: u32) -> Option<std::path::PathBuf> {
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer's length is passed.
    let length = unsafe {
        libc::proc_pidpath(
            i32::try_from(pid).ok()?,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
        )
    };
    if length <= 0 {
        return None;
    }
    buffer.truncate(length as usize);
    use std::os::unix::ffi::OsStringExt;
    Some(std::ffi::OsString::from_vec(buffer).into())
}

/// The `opened` reply's native process is the guard's own child, started
/// no earlier than the guard (a reused pid is older than its new parent's
/// child could be), running exactly `program` (the bundle's
/// `Contents/MacOS` executable, compared as the canonical path the App
/// located).
pub(super) fn native_owned(native_pid: u32, guard: &std::process::Child, program: &Path) -> bool {
    let (Some((parent, started)), Some((_, guard_started))) =
        (parent_and_start(native_pid), parent_and_start(guard.id()))
    else {
        return false;
    };
    parent == guard.id()
        && started >= guard_started
        && image_of(native_pid).as_deref() == Some(program)
}

/// The cache path CEF reported is exactly the requested one (the browser
/// compares the spelling exactly on macOS too).
pub(super) fn same_path(reported: &str, expected: &Path) -> bool {
    std::ffi::OsStr::new(reported) == expected.as_os_str()
}

/// This machine's host name, as Chromium writes it into SingletonLock.
pub(super) fn hostname() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer's length is passed; gethostname terminates it
    // when it fits.
    if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } != 0 {
        return String::new();
    }
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

/// Whether a process id names a process now (also another user's).
pub(super) fn process_exists(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return true;
    };
    // SAFETY: signal 0 only checks that the process exists.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// The person's default browser through `/usr/bin/open`, in its own process
/// group (out of the App's terminal signals), waited for here. A fixture
/// build never opens anything: it appends the link to the recording log
/// (`TAKOSUMI_BROWSER_PROCESS_EXTERNAL_LOG`) instead.
#[cfg(all(not(test), feature = "fixture-overrides"))]
pub(super) fn open_url(url: &str) -> bool {
    use super::process_setup::{environment, opener_log, opener_record};
    use std::io::Write;
    let Some(log) = opener_log(&environment) else {
        return false;
    };
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .and_then(|mut file| file.write_all(opener_record(url, std::process::id()).as_bytes()))
        .is_ok()
}
#[cfg(all(not(test), not(feature = "fixture-overrides")))]
pub(super) fn open_url(url: &str) -> bool {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    Command::new("/usr/bin/open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .status()
        .is_ok_and(|status| status.success())
}
