//! macOS pieces of the per-profile browser adapter (`process_adapter.rs`,
//! M2): the identity checks on a browser's `opened` reply, Chromium's profile
//! lock and the external-link opener. The rest is POSIX and shared with
//! Linux. libproc (`proc_pidinfo`, `proc_pidpath`) through libc.
use std::path::{Path, PathBuf};

#[path = "process_image.rs"]
mod image;
pub(super) use image::Executable;

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
/// child could be). Its image must name the file held before spawn, including
/// Chromium's hard-linked code-sign clone. Both process identities and the
/// image path must remain stable across the file check. proc_pidpath + open
/// is not an atomic running-vnode query; same-user concurrent file mutation
/// and bundle authentication remain outside this child-ownership check.
pub(super) fn native_owned(
    native_pid: u32,
    guard: &std::process::Child,
    program: &Path,
    executable: &Executable,
) -> bool {
    let native = parent_and_start(native_pid);
    let parent = parent_and_start(guard.id());
    let parent_matches = native.is_some_and(|(pid, _)| pid == guard.id());
    let birth_order = native
        .zip(parent)
        .is_some_and(|((_, started), (_, guard_started))| started >= guard_started);
    // Preserve the original short circuit, and report the very same reads
    // used for admission. A second diagnostic read could hide a transient.
    let image_checked = parent_matches && birth_order;
    let image = image_checked.then(|| image_of(native_pid)).flatten();
    let image_matches = image
        .as_deref()
        .is_some_and(|path| executable.matches(path));
    let identities_unchanged = image_matches
        && image_of(native_pid) == image
        && parent_and_start(native_pid) == native
        && parent_and_start(guard.id()) == parent;
    let owned = image_checked && image_matches && identities_unchanged;
    if !owned {
        use std::io::Write;
        let _ = writeln!(
            std::io::stderr().lock(),
            "takosumi-mac-native-rejected: {}",
            serde_json::json!({
                "nativeIdentityRead":native.is_some(),"guardIdentityRead":parent.is_some(),
                "parentMatch":parent_matches,"birthOrder":birth_order,"imageChecked":image_checked,
                "imageRead":image.is_some(),"imageMatch":image_matches,
                "identitiesUnchanged":identities_unchanged,
                "exactPathMatch":image.as_deref()==Some(program)
            })
        );
        #[cfg(any(test, feature = "fixture-overrides"))]
        describe_rejected_image(
            native_pid,
            guard.id(),
            native,
            parent,
            image.as_deref(),
            program,
        );
    }
    owned
}

/// Anonymous native fixtures only: keep the exact paths from the failed
/// comparison so a spelling difference can be distinguished from a different
/// executable (non-UTF-8 paths are null, never a serialization panic).
/// These later filesystem/process reads are diagnostic, never a
/// fallback admission check. Product builds retain only the booleans above.
#[cfg(any(test, feature = "fixture-overrides"))]
fn describe_rejected_image(
    native_pid: u32,
    guard_pid: u32,
    native: Option<(u32, u64)>,
    parent: Option<(u32, u64)>,
    image: Option<&Path>,
    program: &Path,
) {
    use std::io::Write;
    use std::os::unix::fs::MetadataExt;
    let canonical = image.and_then(|path| path.canonicalize().ok());
    let same_file = image
        .and_then(|path| path.metadata().ok())
        .zip(program.metadata().ok())
        .map(|(actual, expected)| {
            actual.is_file()
                && expected.is_file()
                && actual.dev() == expected.dev()
                && actual.ino() == expected.ino()
        });
    let _ = writeln!(
        std::io::stderr().lock(),
        "takosumi-mac-fixture-image-rejected: {}",
        serde_json::json!({
            "nativePid":native_pid,"guardPid":guard_pid,
            "expectedPathUtf8":program.to_str(),"observedPathUtf8":image.and_then(Path::to_str),
            "canonicalMatch":canonical.as_deref().map(|path| path==program),
            "sameFile":same_file,
            "identitiesUnchanged":native.is_some() && parent.is_some()
                && parent_and_start(native_pid)==native && parent_and_start(guard_pid)==parent
        })
    );
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

/// The system's per-user temporary directory (`confstr`,
/// `_CS_DARWIN_USER_TEMP_DIR`, such as `/var/folders/…/T/`). On macOS
/// Chromium takes its temporary directory from `NSTemporaryDirectory()`,
/// which prefers this one (only `MAC_CHROMIUM_TMPDIR` overrides it), so
/// the `TMPDIR` the browser is given is informational: this directory is
/// where its process-singleton socket goes, at
/// `<dir>/.<base bundle id>.XXXXXX/SingletonSocket` (base's macOS
/// temporary-file template), which must fit `sockaddr_un` (104 bytes on
/// macOS, its terminator included). A directory too long for it is refused
/// with that reason.
pub(super) fn browser_temp_dir() -> crate::Result<PathBuf> {
    let mut buffer = vec![0u8; libc::PATH_MAX as usize + 1];
    // SAFETY: the buffer's length is passed; confstr terminates what fits.
    let needed = unsafe {
        libc::confstr(
            libc::_CS_DARWIN_USER_TEMP_DIR,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    if needed <= 1 || needed > buffer.len() {
        return Err(crate::Error::InvalidRequest(
            "The system's per-user temporary directory is unavailable.",
        ));
    }
    buffer.truncate(needed - 1);
    use std::os::unix::ffi::OsStringExt;
    let directory = PathBuf::from(std::ffi::OsString::from_vec(buffer));
    if !directory.is_absolute() || !singleton_socket_fits(&directory) {
        return Err(crate::Error::InvalidRequest(
            "The system's per-user temporary directory is too long for the browser's singleton socket.",
        ));
    }
    Ok(directory)
}

/// The browser bundle's identifier (`scripts/bundle-macos-cef.mjs`,
/// `IDENTIFIER`), which base takes as its bundle id, and Chromium's own
/// default; the socket check uses the longer.
const BROWSER_BUNDLE_ID: &str = "com.takosumi.cef-browser";
const CHROMIUM_BUNDLE_ID: &str = "org.chromium.Chromium";

/// Whether Chromium's singleton socket under `temp`
/// (`<temp>/.<bundle id>.XXXXXX/SingletonSocket`) fits `sockaddr_un`.
pub(super) fn singleton_socket_fits(temp: &Path) -> bool {
    [BROWSER_BUNDLE_ID, CHROMIUM_BUNDLE_ID].iter().all(|id| {
        temp.join(format!(".{id}.XXXXXX/SingletonSocket"))
            .as_os_str()
            .len()
            < 104
    })
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
