//! Where the Linux per-profile browser, its guard and its short runtime root
//! are, and whether the browser runs in Chromium's sandbox. A packaged App
//! finds the browser in its own resources (`cef/`) and keeps sockets under
//! `$XDG_RUNTIME_DIR`; the native fixture names each one explicitly. The
//! decision is a pure function of its inputs; `locate` then checks the actual
//! files and directories before anything is started. The Windows checks of
//! the same plan are in `process_setup_windows.rs`.
use super::{failure, private_directory};
use crate::{Error, Result};
#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt;
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

#[cfg(windows)]
#[path = "process_setup_windows.rs"]
mod windows;
#[cfg(windows)]
pub(super) use windows::{canonical, locate, lock_runtime, sweep_runtime, Identity};
#[cfg(windows)]
use windows::{spelled, stale_lock};

/// The packaged browser's directory inside the App's resource directory.
pub(super) const PACKAGED: &str = "cef";
#[cfg(target_os = "linux")]
pub(super) const BROWSER: &str = "takosumi-cef-browser-process";
#[cfg(target_os = "linux")]
pub(super) const GUARD: &str = "takosumi-browser-guard";
#[cfg(windows)]
pub(super) const BROWSER: &str = "takosumi-cef-browser-process.exe";
#[cfg(windows)]
pub(super) const GUARD: &str = "takosumi-browser-guard.exe";
/// This App's entry in `$XDG_RUNTIME_DIR`.
#[cfg(target_os = "linux")]
const RUNTIME: &str = "takosumi-cef";
/// Its entry in the App's cache directory when no usable runtime directory exists.
const CACHE_RUNTIME: &str = "cef";
/// Only the native fixture's build honours the environment seams below.
const FIXTURE: bool = cfg!(feature = "fixture-overrides");
/// A fixture build keeps an ended browser's runtime directory: its runner
/// reads each guard's receipt there after the App exits. A product build
/// removes it; both sweep directories of ended App instances at start.
pub(super) const KEEP_ENDED_RUNTIME: bool = FIXTURE;

/// Reads one environment variable. The App passes `environment`; tests pass
/// their own table, so they never change the process environment.
pub(super) type Lookup<'a> = &'a dyn Fn(&str) -> Option<OsString>;
pub(super) fn environment(key: &str) -> Option<OsString> {
    std::env::var_os(key)
}

/// Explicit overrides, read once from the App's environment. The native
/// fixture sets all three; a packaged App sets none.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Overrides {
    pub program: Option<OsString>,
    pub guard: Option<OsString>,
    pub runtime: Option<OsString>,
}
impl Overrides {
    /// Only a fixture build honours these; a product build always resolves
    /// its own bundled files.
    pub(super) fn from_lookup(lookup: Lookup) -> Self {
        if !FIXTURE {
            return Self::default();
        }
        Self {
            program: lookup("TAKOSUMI_BROWSER_PROCESS_PROGRAM"),
            guard: lookup("TAKOSUMI_BROWSER_PROCESS_GUARD"),
            runtime: lookup("TAKOSUMI_BROWSER_PROCESS_RUNTIME"),
        }
    }
}

/// The fixture's sandbox opt-out (`TAKOSUMI_BROWSER_PROCESS_NO_SANDBOX`); a
/// product build never reads it. Linux only: on Windows the browser decides
/// its sandbox itself and the App passes no switch.
#[cfg(target_os = "linux")]
pub(super) fn sandbox_opt_out(lookup: Lookup) -> Option<OsString> {
    FIXTURE
        .then(|| lookup("TAKOSUMI_BROWSER_PROCESS_NO_SANDBOX"))
        .flatten()
}

/// Whether the fixture asked for its loopback inspection seam; never in a
/// product build.
pub(super) fn inspection(lookup: Lookup) -> bool {
    FIXTURE && lookup("TAKOSUMI_BROWSER_PROCESS_INSPECT").as_deref() == Some(OsStr::new("1"))
}

/// One line of a fixture build's recording opener log (Windows; see
/// `process_windows::shell_open`): the link as the opener's only argument
/// and the App's pid, as one JSON object.
#[cfg(any(
    all(test, target_os = "linux"),
    all(windows, not(test), feature = "fixture-overrides")
))]
pub(super) fn opener_record(url: &str, pid: u32) -> String {
    serde_json::json!({"argv": [url], "pid": pid}).to_string() + "\n"
}

/// The recording log a fixture build appends to
/// (`TAKOSUMI_BROWSER_PROCESS_EXTERNAL_LOG`), only as an absolute path.
#[cfg(any(
    all(test, target_os = "linux"),
    all(windows, not(test), feature = "fixture-overrides")
))]
pub(super) fn opener_log(lookup: Lookup) -> Option<PathBuf> {
    lookup("TAKOSUMI_BROWSER_PROCESS_EXTERNAL_LOG")
        .map(PathBuf::from)
        .filter(|log| log.is_absolute())
}

/// A setuid or setgid App would start browsers with an identity the person
/// did not choose; the browser path refuses to run that way.
#[cfg(target_os = "linux")]
pub(super) fn plain_identity(uid: u32, euid: u32, gid: u32, egid: u32) -> Result<()> {
    if uid != euid || gid != egid {
        return Err(Error::InvalidRequest(
            "The desktop browser does not run from a setuid or setgid App.",
        ));
    }
    Ok(())
}

/// A place the App may keep its browser runtime root, tried in order.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Candidate {
    /// `$XDG_RUNTIME_DIR/takosumi-cef`, only when the runtime directory itself
    /// follows the XDG rules (owned by this user, mode 0700).
    Xdg(PathBuf),
    /// `<app cache>/cef`, created when missing.
    Cache(PathBuf),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Runtime {
    /// The fixture's own directory: it must already exist.
    Given(PathBuf),
    /// The first candidate that is (or can be made) this user's private root.
    Search(Vec<Candidate>),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Plan {
    pub program: PathBuf,
    pub guard: PathBuf,
    pub runtime: Runtime,
    /// Windows: `<resources>\cef` when the browser or the guard comes from
    /// the App's bundle. Each such executable must then be a plain file
    /// directly in that plain directory (`process_setup_windows.rs`).
    #[cfg(windows)]
    pub bundle: Option<PathBuf>,
}

/// Decide where the browser, its guard and the runtime root are, without
/// touching the filesystem. Each explicit override wins; otherwise the browser
/// and guard come from `<resources>/cef/`, and the runtime root is searched in
/// `$XDG_RUNTIME_DIR` (when set, non-empty and absolute) and then in the App's
/// cache directory (when absolute). Relative directories are never resolved
/// against the working directory.
pub(super) fn plan(
    overrides: &Overrides,
    resources: Option<&Path>,
    xdg_runtime: Option<&OsStr>,
    cache: Option<&Path>,
) -> Result<Plan> {
    let packaged = |name: &str| {
        resources
            .map(|resources| resources.join(PACKAGED).join(name))
            .ok_or(Error::InvalidRequest(
                "The App's resource directory is unavailable; its browser cannot be found.",
            ))
    };
    let program = match &overrides.program {
        Some(path) => PathBuf::from(path),
        None => packaged(BROWSER)?,
    };
    let guard = match &overrides.guard {
        Some(path) => PathBuf::from(path),
        None => packaged(GUARD)?,
    };
    let runtime = match &overrides.runtime {
        Some(path) => Runtime::Given(PathBuf::from(path)),
        None => {
            let xdg = xdg_runtime
                .filter(|dir| !dir.is_empty())
                .map(Path::new)
                .filter(|dir| dir.is_absolute())
                .map(|dir| Candidate::Xdg(dir.to_path_buf()));
            let cache = cache
                .filter(|cache| cache.is_absolute())
                .map(|cache| Candidate::Cache(cache.to_path_buf()));
            Runtime::Search(xdg.into_iter().chain(cache).collect())
        }
    };
    #[cfg(windows)]
    let bundle = (overrides.program.is_none() || overrides.guard.is_none())
        .then(|| resources.map(|resources| resources.join(PACKAGED)))
        .flatten();
    Ok(Plan {
        program,
        guard,
        runtime,
        #[cfg(windows)]
        bundle,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Located {
    pub program: PathBuf,
    pub guard: PathBuf,
    pub runtime: PathBuf,
}

/// Check the planned files and directories: both executables must resolve to
/// existing files, and the runtime root must be private to `euid`. A fixture's
/// root must already be so; otherwise the first usable candidate wins. Every
/// returned path is canonical.
#[cfg(target_os = "linux")]
pub(super) fn locate(plan: Plan, euid: u32) -> Result<Located> {
    let program = existing_file(&plan.program, "Browser process executable is unavailable.")?;
    let guard = existing_file(&plan.guard, "Browser guard is unavailable.")?;
    let runtime = match plan.runtime {
        Runtime::Given(path) => private_runtime(&path, euid)?,
        Runtime::Search(candidates) => search(&candidates, euid)?,
    };
    Ok(Located {
        program,
        guard,
        runtime,
    })
}

fn existing_file(path: &Path, missing: &'static str) -> Result<PathBuf> {
    let canonical = path
        .canonicalize()
        .map(spelled)
        .map_err(|error| failure(format!("{missing} ({}: {error})", path.display())))?;
    if !canonical.is_file() {
        return Err(Error::InvalidRequest(missing));
    }
    Ok(canonical)
}

/// The canonical path in the spelling the browser is given. On Linux that is
/// `canonicalize`'s own result; Windows drops its verbatim prefix.
#[cfg(target_os = "linux")]
fn spelled(canonical: PathBuf) -> PathBuf {
    canonical
}
#[cfg(target_os = "linux")]
pub(super) fn canonical(path: &Path) -> Result<PathBuf> {
    path.canonicalize().map(spelled).map_err(failure)
}

/// What `lstat` showed about one path.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Seen {
    pub dir: bool,
    pub uid: u32,
    pub mode: u32,
}
#[cfg(target_os = "linux")]
fn seen(path: &Path) -> Result<Option<Seen>> {
    match path.symlink_metadata() {
        Ok(metadata) => Ok(Some(Seen {
            dir: metadata.file_type().is_dir(),
            uid: metadata.uid(),
            mode: metadata.mode() & 0o7777,
        })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(failure(error)),
    }
}

/// The XDG Base Directory rules for `$XDG_RUNTIME_DIR`: a directory owned by
/// the user with mode 0700. Another user's directory (after `su`) or a shared
/// `/tmp` (1777) does not qualify.
#[cfg(target_os = "linux")]
pub(super) fn xdg_usable(seen: Option<Seen>, euid: u32) -> bool {
    matches!(seen, Some(seen) if seen.dir && seen.uid == euid && seen.mode & 0o777 == 0o700)
}

/// An existing runtime root this App may use: a plain directory (not a link)
/// of this user, closed to group and others. Another owner's (one left by
/// `sudo -E`) or a looser one is skipped, never chmod-ed or chown-ed.
#[cfg(target_os = "linux")]
pub(super) fn root_usable(seen: Option<Seen>, euid: u32) -> bool {
    matches!(seen, Some(seen) if seen.dir && seen.uid == euid && seen.mode & 0o077 == 0)
}

#[cfg(target_os = "linux")]
fn search(candidates: &[Candidate], euid: u32) -> Result<PathBuf> {
    let mut reasons = Vec::new();
    for candidate in candidates {
        match usable_candidate(candidate, euid) {
            Ok(root) => return Ok(root),
            Err(error) => {
                let (Candidate::Xdg(place) | Candidate::Cache(place)) = candidate;
                reasons.push(format!("{}: {error}", place.display()));
            }
        }
    }
    Err(failure(format!(
        "No private runtime directory is available for the browser{}",
        if reasons.is_empty() {
            String::new()
        } else {
            format!(" ({})", reasons.join("; "))
        }
    )))
}

#[cfg(target_os = "linux")]
fn usable_candidate(candidate: &Candidate, euid: u32) -> Result<PathBuf> {
    let root = match candidate {
        Candidate::Xdg(dir) => {
            if !xdg_usable(seen(dir)?, euid) {
                return Err(Error::InvalidRequest(
                    "XDG_RUNTIME_DIR is not this user's private (0700) runtime directory.",
                ));
            }
            dir.join(RUNTIME)
        }
        Candidate::Cache(cache) => cache.join(CACHE_RUNTIME),
    };
    match seen(&root)? {
        None => private_directory(&root)?,
        found if !root_usable(found, euid) => {
            return Err(Error::InvalidRequest(
                "The browser runtime directory there is not this user's private directory.",
            ))
        }
        Some(_) => {}
    }
    // Checked again after any creation: another process may have raced it.
    private_runtime(&root, euid)
}

/// The runtime root holds each browser's sockets and temporary files: a plain
/// directory (not a link) owned by `euid`, closed to group and others, and
/// short enough that Chromium's singleton socket fits `sockaddr_un` for every
/// runtime name this App instance could plausibly use.
#[cfg(target_os = "linux")]
fn private_runtime(path: &Path, euid: u32) -> Result<PathBuf> {
    let metadata = path.symlink_metadata().map_err(failure)?;
    if !metadata.file_type().is_dir() {
        return Err(Error::InvalidRequest(
            "Browser runtime directory must be a plain directory.",
        ));
    }
    if metadata.uid() != euid {
        return Err(Error::InvalidRequest(
            "Browser runtime directory must belong to this user.",
        ));
    }
    if metadata.mode() & 0o077 != 0 {
        return Err(Error::InvalidRequest(
            "Browser runtime directory must be private to this user.",
        ));
    }
    let canonical = path.canonicalize().map_err(failure)?;
    if !socket_path_fits(&canonical.join(runtime_name(std::process::id(), 999_999))) {
        return Err(Error::InvalidRequest(
            "Browser runtime directory path is too long.",
        ));
    }
    Ok(canonical)
}

/// Chromium's temporary singleton socket under a browser's runtime directory
/// must fit `sockaddr_un` (108 bytes including the terminator). Linux only:
/// Chromium on Windows keeps no socket there.
#[cfg(target_os = "linux")]
pub(super) fn socket_path_fits(runtime: &Path) -> bool {
    runtime
        .join("tmp/org.chromium.Chromium.XXXXXX/SingletonSocket")
        .as_os_str()
        .len()
        < 108
}

/// One browser's runtime directory inside the root: `p-<App pid>-<n>`.
pub(super) fn runtime_name(pid: u32, n: u64) -> String {
    format!("p-{pid}-{n}")
}
/// Whether a name is a browser runtime directory's (`p-<pid>-<n>`).
fn is_runtime_name(name: &str) -> bool {
    name.strip_prefix("p-")
        .and_then(|rest| rest.split_once('-'))
        .is_some_and(|(pid, n)| pid.parse::<u32>().is_ok() && n.parse::<u64>().is_ok())
}

/// The lock file inside each browser runtime directory. The App holds an
/// exclusive `flock` on it for that browser's whole lifetime, so liveness is
/// decided by the lock, not by a pid: another host sharing an NFS cache, or
/// another PID namespace sharing `/run/user/<uid>`, keeps its directories.
const RUNTIME_LOCK: &str = "lock";

/// Create and lock a fresh runtime directory's lock file. The returned file
/// holds the lock until it is closed; it is never inherited by the guard
/// (close-on-exec).
#[cfg(target_os = "linux")]
pub(super) fn lock_runtime(runtime: &Path) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(runtime.join(RUNTIME_LOCK))
        .map_err(failure)?;
    match try_lock(&file) {
        Ok(true) => Ok(file),
        Ok(false) => Err(Error::InvalidRequest(
            "Browser runtime directory is locked by another App instance.",
        )),
        Err(error) => Err(failure(error)),
    }
}

/// A non-blocking exclusive `flock`: false when another open file description
/// holds it.
#[cfg(target_os = "linux")]
fn try_lock(file: &std::fs::File) -> std::io::Result<bool> {
    use std::os::fd::AsRawFd;
    // SAFETY: a live descriptor owned by `file`.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(false)
    } else {
        Err(error)
    }
}

/// The lock of an existing runtime directory, if this App can take it now:
/// `None` while another instance holds it, and also when there is no lock
/// file (a directory being created, or one from an earlier build), which is
/// never judged stale. The lock file is never followed as a link.
#[cfg(target_os = "linux")]
fn stale_lock(runtime: &Path) -> Result<Option<std::fs::File>> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(runtime.join(RUNTIME_LOCK))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(failure(error)),
    };
    Ok(try_lock(&file).map_err(failure)?.then_some(file))
}

/// Make `runtime` free for a new browser: true when it is missing, or was a
/// stale directory (its lock taken) and has been removed; false when another
/// App instance holds it or it has no lock file to judge by.
pub(super) fn claim_runtime(runtime: &Path) -> Result<bool> {
    match runtime.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(failure(error)),
        Ok(metadata) if !metadata.file_type().is_dir() => Err(Error::InvalidRequest(
            "Browser runtime entry is not a plain directory.",
        )),
        Ok(_) => match stale_lock(runtime)? {
            // Removed while its lock is held, so no other App can claim it meanwhile.
            Some(_lock) => remove_runtime(runtime).map(|()| true),
            None => Ok(false),
        },
    }
}

/// Remove one browser runtime directory. Only a plain directory is removed;
/// a link or file in its place is refused, and std's `remove_dir_all` never
/// follows links inside it.
pub(super) fn remove_runtime(path: &Path) -> Result<()> {
    match path.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_dir() => {
            std::fs::remove_dir_all(path).map_err(failure)
        }
        Ok(_) => Err(Error::InvalidRequest(
            "Browser runtime entry is not a plain directory.",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(failure(error)),
    }
}

/// Inside the verified runtime root, remove browser runtime directories whose
/// lock no App instance holds: those of ended instances, on this host or any
/// other sharing the root. Only plain directories of `euid` named
/// `p-<pid>-<n>` with a lock file are considered; a held lock, a missing lock
/// file, links and other entries stay. Returns the failures; none stops the
/// App.
#[cfg(target_os = "linux")]
pub(super) fn sweep_runtime(root: &Path, euid: u32) -> Vec<String> {
    sweep(root, |metadata| {
        metadata.file_type().is_dir() && metadata.uid() == euid
    })
}
/// The sweep itself; `is_ours` decides which `lstat` results are this user's
/// plain directories.
fn sweep(root: &Path, is_ours: impl Fn(&std::fs::Metadata) -> bool) -> Vec<String> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => return vec![error.to_string()],
    };
    let mut problems = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                problems.push(error.to_string());
                continue;
            }
        };
        if !entry.file_name().to_str().is_some_and(is_runtime_name) {
            continue;
        }
        let path = entry.path();
        let ours = path
            .symlink_metadata()
            .is_ok_and(|metadata| is_ours(&metadata));
        if !ours {
            continue;
        }
        let result = stale_lock(&path).and_then(|lock| match lock {
            // Removed while its lock is held, so no other App can claim it meanwhile.
            Some(_lock) => remove_runtime(&path),
            None => Ok(()),
        });
        if let Err(error) = result {
            problems.push(format!("{}: {error}", path.display()));
        }
    }
    problems
}

/// Chromium refuses its sandbox to root, so a root App (the native fixture
/// runs as root) starts browsers with `--no-sandbox`. Otherwise only an
/// explicit `TAKOSUMI_BROWSER_PROCESS_NO_SANDBOX=1` (fixture builds only, see
/// `sandbox_opt_out`) opts out. The browser reads the same decision from its
/// arguments.
#[cfg(target_os = "linux")]
pub(super) fn without_sandbox(euid: u32, opt_out: Option<&OsStr>) -> bool {
    euid == 0 || opt_out == Some(OsStr::new("1"))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn overrides(program: Option<&str>, guard: Option<&str>, runtime: Option<&str>) -> Overrides {
        Overrides {
            program: program.map(OsString::from),
            guard: guard.map(OsString::from),
            runtime: runtime.map(OsString::from),
        }
    }
    fn euid() -> u32 {
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() }
    }
    /// A fresh private directory for one test, removed by the test itself.
    /// Names stay short: a runtime root below it must still leave room for
    /// Chromium's socket path (TMPDIR as short as /root/dev/tmp).
    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ps{name}{}", std::process::id()));
        private_directory(&root).unwrap();
        root
    }
    fn mode(path: &Path) -> u32 {
        std::fs::symlink_metadata(path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    }
    fn chmod(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    /// Every fixture variable set, as a hostile or leftover environment would.
    fn everything(key: &str) -> Option<OsString> {
        Some(OsString::from(match key {
            "TAKOSUMI_BROWSER_PROCESS_NO_SANDBOX" | "TAKOSUMI_BROWSER_PROCESS_INSPECT" => "1",
            _ => "/elsewhere",
        }))
    }

    #[test]
    fn the_opener_record_is_one_json_line_with_only_the_link_and_the_app_pid() {
        let url = "https://example.com/a?b=\"c\"";
        let line = opener_record(url, 4242);
        assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&line).unwrap(),
            serde_json::json!({"argv": [url], "pid": 4242})
        );
    }

    #[test]
    fn only_an_absolute_opener_log_is_used() {
        let named = |value: &'static str| {
            move |key: &str| {
                (key == "TAKOSUMI_BROWSER_PROCESS_EXTERNAL_LOG").then(|| OsString::from(value))
            }
        };
        assert_eq!(
            opener_log(&named("/case/external-open.log")),
            Some(PathBuf::from("/case/external-open.log"))
        );
        assert_eq!(opener_log(&named("external-open.log")), None);
        assert_eq!(opener_log(&|_: &str| None), None);
    }

    #[test]
    fn a_packaged_app_finds_its_browser_in_resources_and_searches_runtime_then_cache() {
        let packaged = plan(
            &Overrides::default(),
            Some(Path::new("/usr/lib/Takosumi")),
            Some(OsStr::new("/run/user/1000")),
            Some(Path::new("/home/a/.cache/com.takosumi")),
        )
        .unwrap();
        assert_eq!(
            packaged,
            Plan {
                program: "/usr/lib/Takosumi/cef/takosumi-cef-browser-process".into(),
                guard: "/usr/lib/Takosumi/cef/takosumi-browser-guard".into(),
                runtime: Runtime::Search(vec![
                    Candidate::Xdg("/run/user/1000".into()),
                    Candidate::Cache("/home/a/.cache/com.takosumi".into()),
                ]),
            }
        );
    }

    #[test]
    fn explicit_fixture_overrides_win_and_need_no_resources() {
        let fixture = plan(
            &overrides(
                Some("/case/browser"),
                Some("/case/guard.py"),
                Some("/case/r"),
            ),
            None,
            Some(OsStr::new("/run/user/0")),
            None,
        )
        .unwrap();
        assert_eq!(
            fixture,
            Plan {
                program: "/case/browser".into(),
                guard: "/case/guard.py".into(),
                runtime: Runtime::Given("/case/r".into()),
            }
        );
        // Each override stands alone; the rest still comes from the package.
        let mixed = plan(
            &overrides(None, Some("/case/guard.py"), None),
            Some(Path::new("/opt/x")),
            Some(OsStr::new("/run/user/7")),
            None,
        )
        .unwrap();
        assert_eq!(
            mixed.program,
            PathBuf::from("/opt/x/cef/takosumi-cef-browser-process")
        );
        assert_eq!(mixed.guard, PathBuf::from("/case/guard.py"));
        assert_eq!(
            mixed.runtime,
            Runtime::Search(vec![Candidate::Xdg("/run/user/7".into())])
        );
    }

    #[test]
    fn unset_empty_or_relative_directories_are_never_candidates() {
        let cache = Some(Path::new("/home/a/.cache/com.takosumi"));
        let only_cache =
            Runtime::Search(vec![Candidate::Cache("/home/a/.cache/com.takosumi".into())]);
        for xdg in [None, Some(OsStr::new("")), Some(OsStr::new("relative/run"))] {
            let chosen = plan(&Overrides::default(), Some(Path::new("/r")), xdg, cache).unwrap();
            assert_eq!(chosen.runtime, only_cache, "{xdg:?}");
        }
        let relative_cache = plan(
            &Overrides::default(),
            Some(Path::new("/r")),
            None,
            Some(Path::new("relative/cache")),
        )
        .unwrap();
        assert_eq!(relative_cache.runtime, Runtime::Search(Vec::new()));
        // With no candidate at all, locating reports that nothing is available.
        let root = scratch("n");
        std::fs::write(root.join("b"), b"").unwrap();
        let located = locate(
            Plan {
                program: root.join("b"),
                guard: root.join("b"),
                runtime: Runtime::Search(Vec::new()),
            },
            euid(),
        );
        std::fs::remove_dir_all(&root).unwrap();
        assert!(located.is_err());
    }

    #[test]
    fn without_resources_or_overrides_there_is_no_browser() {
        let run = Some(OsStr::new("/run/user/1000"));
        assert!(plan(&Overrides::default(), None, run, None).is_err());
        assert!(plan(&overrides(Some("/b"), None, None), None, run, None).is_err());
        assert!(plan(&overrides(None, Some("/g"), None), None, run, None).is_err());
    }

    #[test]
    fn xdg_runtime_dirs_follow_the_xdg_rules_and_existing_roots_must_be_ours() {
        let me = 1000;
        let at = |dir, uid, mode| Some(Seen { dir, uid, mode });
        assert!(xdg_usable(at(true, me, 0o700), me));
        for (case, seen) in [
            ("missing", None),
            ("another user's, after su", at(true, 1001, 0o700)),
            ("root's, left by sudo -E", at(true, 0, 0o700)),
            ("a shared /tmp", at(true, 0, 0o1777)),
            ("group readable", at(true, me, 0o750)),
            ("not a directory", at(false, me, 0o700)),
        ] {
            assert!(!xdg_usable(seen, me), "{case}");
        }
        assert!(root_usable(at(true, me, 0o700), me));
        for (case, seen) in [
            ("missing", None),
            ("root-owned, left by sudo -E", at(true, 0, 0o700)),
            ("another user's", at(true, 1001, 0o700)),
            ("open to others", at(true, me, 0o755)),
            ("a link or file", at(false, me, 0o700)),
        ] {
            assert!(!root_usable(seen, me), "{case}");
        }
    }

    #[test]
    fn an_unusable_runtime_dir_falls_back_to_the_cache_and_is_left_untouched() {
        let root = scratch("f");
        std::fs::write(root.join("b"), b"").unwrap();
        let locate_with = |xdg: &Path, cache: &Path| {
            locate(
                Plan {
                    program: root.join("b"),
                    guard: root.join("b"),
                    runtime: Runtime::Search(vec![
                        Candidate::Xdg(xdg.to_owned()),
                        Candidate::Cache(cache.to_owned()),
                    ]),
                },
                euid(),
            )
        };
        let cache_root = root.join("c").join(CACHE_RUNTIME);
        // The runtime directory itself is not 0700 (a shared or loose one).
        private_directory(&root.join("loose")).unwrap();
        chmod(&root.join("loose"), 0o755);
        let loose = locate_with(&root.join("loose"), &root.join("c"));
        let loose_child = root.join("loose").join(RUNTIME).exists();
        // A usable runtime directory whose takosumi-cef is not private: skipped,
        // never repaired.
        private_directory(&root.join("x")).unwrap();
        private_directory(&root.join("x").join(RUNTIME)).unwrap();
        chmod(&root.join("x").join(RUNTIME), 0o755);
        let foreign = locate_with(&root.join("x"), &root.join("c"));
        let foreign_mode = mode(&root.join("x").join(RUNTIME));
        // A usable runtime directory too long for the socket path.
        let long = root.join("l".repeat(60));
        private_directory(&long).unwrap();
        let too_long = locate_with(&long, &root.join("c"));
        // A missing runtime directory is not created on its behalf.
        let missing = locate_with(&root.join("absent"), &root.join("c"));
        let absent_created = root.join("absent").exists();
        let canonical = cache_root.canonicalize().unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        for (case, result) in [
            ("loose", loose),
            ("foreign root", foreign),
            ("too long", too_long),
            ("missing", missing),
        ] {
            assert_eq!(result.unwrap().runtime, canonical, "{case}");
        }
        assert!(!loose_child, "nothing is created in a loose runtime dir");
        assert_eq!(foreign_mode, 0o755, "a skipped root keeps its mode");
        assert!(!absent_created);
    }

    #[test]
    fn when_every_candidate_fails_locating_names_each_reason() {
        let root = scratch("e");
        std::fs::write(root.join("b"), b"").unwrap();
        private_directory(&root.join("loose")).unwrap();
        chmod(&root.join("loose"), 0o755);
        // The cache fallback is too long for the socket path.
        let cache = root.join("c".repeat(60));
        let located = locate(
            Plan {
                program: root.join("b"),
                guard: root.join("b"),
                runtime: Runtime::Search(vec![
                    Candidate::Xdg(root.join("loose")),
                    Candidate::Cache(cache.clone()),
                ]),
            },
            euid(),
        );
        std::fs::remove_dir_all(&root).unwrap();
        let message = located.unwrap_err().to_string();
        assert!(message.contains("XDG_RUNTIME_DIR"), "{message}");
        assert!(message.contains("too long"), "{message}");
    }

    #[test]
    fn locating_a_packaged_layout_creates_a_private_runtime_and_returns_canonical_paths() {
        let root = scratch("l");
        let cef = root.join("resources").join(PACKAGED);
        private_directory(&cef).unwrap();
        for name in [BROWSER, GUARD] {
            std::fs::write(cef.join(name), b"").unwrap();
        }
        private_directory(&root.join("x")).unwrap();
        let planned = plan(
            &Overrides::default(),
            Some(root.join("resources/.").as_path()),
            Some(root.join("x").as_os_str()),
            None,
        )
        .unwrap();
        let located = locate(planned, euid()).unwrap();
        let canonical = root.canonicalize().unwrap();
        let created = mode(&root.join("x").join(RUNTIME));
        // Locating again accepts the directory it created.
        let again = locate(
            plan(
                &Overrides::default(),
                Some(root.join("resources").as_path()),
                Some(root.join("x").as_os_str()),
                None,
            )
            .unwrap(),
            euid(),
        );
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(
            located,
            Located {
                program: canonical.join("resources/cef").join(BROWSER),
                guard: canonical.join("resources/cef").join(GUARD),
                runtime: canonical.join("x").join(RUNTIME),
            }
        );
        assert_eq!(created, 0o700);
        assert_eq!(again.unwrap(), located);
    }

    #[test]
    fn a_missing_or_non_file_executable_is_refused() {
        let root = scratch("m");
        private_directory(&root.join("r")).unwrap();
        private_directory(&root.join("dir")).unwrap();
        std::fs::write(root.join("guard"), b"").unwrap();
        let attempt = |program: &str| {
            locate(
                Plan {
                    program: root.join(program),
                    guard: root.join("guard"),
                    runtime: Runtime::Given(root.join("r")),
                },
                euid(),
            )
        };
        let (missing, directory) = (attempt("absent"), attempt("dir"));
        std::fs::remove_dir_all(&root).unwrap();
        assert!(missing.is_err());
        assert!(directory.is_err());
    }

    #[test]
    fn the_runtime_root_must_be_a_private_plain_short_directory_of_this_user() {
        let root = scratch("r");
        std::fs::write(root.join("guard"), b"").unwrap();
        let attempt = |runtime: Runtime, euid: u32| {
            locate(
                Plan {
                    program: root.join("guard"),
                    guard: root.join("guard"),
                    runtime,
                },
                euid,
            )
        };
        let cache = |base: &str| Runtime::Search(vec![Candidate::Cache(root.join(base))]);
        private_directory(&root.join("ok")).unwrap();
        let canonical = root.join("ok").canonicalize().unwrap();
        let ok = attempt(Runtime::Given(root.join("ok")), euid());
        // Another user's directory, by the check's own identity input.
        let foreign = attempt(Runtime::Given(root.join("ok")), euid().wrapping_add(1));
        // A fixture runtime is never created on its behalf.
        let absent = attempt(Runtime::Given(root.join("absent")), euid());
        private_directory(&root.join("open").join(CACHE_RUNTIME)).unwrap();
        chmod(&root.join("open").join(CACHE_RUNTIME), 0o750);
        let open = attempt(cache("open"), euid());
        private_directory(&root.join("link")).unwrap();
        std::os::unix::fs::symlink(root.join("ok"), root.join("link").join(CACHE_RUNTIME)).unwrap();
        let link = attempt(cache("link"), euid());
        let long = "l".repeat(60);
        let too_long = attempt(cache(&long), euid());
        let long_created = root.join(&long).join(CACHE_RUNTIME).is_dir();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(ok.unwrap().runtime, canonical);
        assert!(foreign.is_err());
        assert!(absent.is_err());
        assert!(open.is_err());
        assert!(link.is_err());
        assert!(too_long.is_err());
        assert!(long_created, "the cache root is created before its checks");
    }

    #[test]
    fn socket_paths_must_fit_sockaddr_un() {
        // `/tmp/org.chromium.Chromium.XXXXXX/SingletonSocket` adds 49 bytes.
        assert!(socket_path_fits(Path::new(&format!("/{}", "a".repeat(57)))));
        assert!(!socket_path_fits(Path::new(&format!(
            "/{}",
            "a".repeat(58)
        ))));
    }

    #[test]
    fn only_root_or_an_explicit_opt_out_runs_without_the_sandbox() {
        assert!(without_sandbox(0, None));
        assert!(without_sandbox(0, Some(OsStr::new("0"))));
        assert!(!without_sandbox(1000, None));
        assert!(without_sandbox(1000, Some(OsStr::new("1"))));
        for other in ["", "0", "true", "yes", " 1"] {
            assert!(!without_sandbox(1000, Some(OsStr::new(other))), "{other:?}");
        }
    }

    #[cfg(not(feature = "fixture-overrides"))]
    #[test]
    fn a_product_build_ignores_every_fixture_variable() {
        assert_eq!(Overrides::from_lookup(&everything), Overrides::default());
        assert_eq!(sandbox_opt_out(&everything), None);
        assert!(!inspection(&everything));
        // So a non-root product App always runs sandboxed.
        assert!(!without_sandbox(
            1000,
            sandbox_opt_out(&everything).as_deref()
        ));
    }

    #[cfg(feature = "fixture-overrides")]
    #[test]
    fn a_fixture_build_reads_its_variables() {
        assert_eq!(
            Overrides::from_lookup(&everything),
            overrides(Some("/elsewhere"), Some("/elsewhere"), Some("/elsewhere"))
        );
        assert_eq!(sandbox_opt_out(&everything), Some(OsString::from("1")));
        assert!(inspection(&everything));
        assert!(without_sandbox(
            1000,
            sandbox_opt_out(&everything).as_deref()
        ));
        let nothing = |_: &str| None;
        assert_eq!(Overrides::from_lookup(&nothing), Overrides::default());
        assert!(!inspection(&nothing));
        let other = |_: &str| Some(OsString::from("0"));
        assert!(!inspection(&other));
    }

    #[test]
    fn a_setuid_or_setgid_app_does_not_start_browsers() {
        assert!(plain_identity(1000, 1000, 1000, 1000).is_ok());
        assert!(plain_identity(0, 0, 0, 0).is_ok());
        assert!(plain_identity(1000, 0, 1000, 1000).is_err(), "setuid root");
        assert!(plain_identity(0, 1000, 0, 0).is_err());
        assert!(plain_identity(1000, 1000, 1000, 0).is_err(), "setgid");
    }

    #[test]
    fn the_sweep_removes_only_unlocked_runtime_dirs_and_never_follows_links() {
        let root = scratch("s");
        let outside = root.join("outside");
        private_directory(&outside.join("kept")).unwrap();
        let runtime = root.join("rt");
        private_directory(&runtime).unwrap();
        // Stale: its lock file exists and nobody holds it. A link inside it is
        // removed, not followed.
        let stale = runtime.join(runtime_name(4_000_002, 1));
        private_directory(&stale.join("tmp")).unwrap();
        drop(lock_runtime(&stale).unwrap());
        std::os::unix::fs::symlink(&outside, stale.join("tmp").join("out")).unwrap();
        // Live: another open file description holds its lock (another App,
        // possibly on another host or in another PID namespace with this pid).
        let live = runtime.join(runtime_name(std::process::id(), 2));
        private_directory(&live).unwrap();
        let held = lock_runtime(&live).unwrap();
        // Not judged: no lock file (being created, or from an earlier build).
        let unjudged = runtime.join(runtime_name(4_000_003, 3));
        private_directory(&unjudged).unwrap();
        // A lock file that is a link is never followed.
        let linked = runtime.join(runtime_name(4_000_004, 4));
        private_directory(&linked).unwrap();
        std::os::unix::fs::symlink(outside.join("kept"), linked.join(RUNTIME_LOCK)).unwrap();
        // Links, files and other names, even with an unlocked lock file.
        std::os::unix::fs::symlink(&outside, runtime.join(runtime_name(4_000_002, 9))).unwrap();
        std::fs::write(runtime.join(runtime_name(4_000_002, 8)), b"").unwrap();
        for other in ["keep", "p-x-1"] {
            private_directory(&runtime.join(other)).unwrap();
            drop(lock_runtime(&runtime.join(other)).unwrap());
        }
        let problems = sweep_runtime(&runtime, euid());
        drop(held);
        let mut left = std::fs::read_dir(&runtime)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        left.sort();
        let outside_kept = outside.join("kept").is_dir();
        std::fs::remove_dir_all(&root).unwrap();
        let mut expected = vec![
            runtime_name(std::process::id(), 2),
            runtime_name(4_000_003, 3),
            runtime_name(4_000_004, 4),
            runtime_name(4_000_002, 8),
            runtime_name(4_000_002, 9),
            "keep".into(),
            "p-x-1".into(),
        ];
        expected.sort();
        assert_eq!(left, expected);
        assert_eq!(problems.len(), 1, "only the linked lock file: {problems:?}");
        assert!(outside_kept, "links are never followed");
    }

    #[test]
    fn a_runtime_name_is_claimed_only_when_missing_or_stale() {
        let root = scratch("c");
        let missing = root.join(runtime_name(4_000_001, 1));
        let stale = root.join(runtime_name(4_000_001, 2));
        private_directory(&stale.join("tmp")).unwrap();
        drop(lock_runtime(&stale).unwrap());
        let held_dir = root.join(runtime_name(4_000_001, 3));
        private_directory(&held_dir).unwrap();
        let held = lock_runtime(&held_dir).unwrap();
        let unjudged = root.join(runtime_name(4_000_001, 4));
        private_directory(&unjudged).unwrap();
        let file = root.join(runtime_name(4_000_001, 5));
        std::fs::write(&file, b"").unwrap();
        let results = [
            claim_runtime(&missing).unwrap(),
            claim_runtime(&stale).unwrap(),
            claim_runtime(&held_dir).unwrap(),
            claim_runtime(&unjudged).unwrap(),
        ];
        let not_a_dir = claim_runtime(&file);
        let (stale_gone, held_kept) = (!stale.exists(), held_dir.join(RUNTIME_LOCK).exists());
        drop(held);
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(results, [true, true, false, false]);
        assert!(not_a_dir.is_err());
        assert!(stale_gone && held_kept);
    }
}
