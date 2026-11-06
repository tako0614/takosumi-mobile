//! Where the Linux per-profile browser, its guard and its short runtime root
//! are, and whether the browser runs in Chromium's sandbox. A packaged App
//! finds the browser in its own resources (`cef/`) and keeps sockets under
//! `$XDG_RUNTIME_DIR`; the native fixture names each one explicitly. The
//! decision is a pure function of its inputs; `locate` then checks the actual
//! files and directories before anything is started.
use super::{failure, private_directory};
use crate::{Error, Result};
use std::{
    ffi::{OsStr, OsString},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// The packaged browser's directory inside the App's resource directory.
pub(super) const PACKAGED: &str = "cef";
pub(super) const BROWSER: &str = "takosumi-cef-browser-process";
pub(super) const GUARD: &str = "takosumi-browser-guard";
/// This App's entry in `$XDG_RUNTIME_DIR`.
const RUNTIME: &str = "takosumi-cef";
/// Its entry in the App's cache directory when no runtime directory is set.
const CACHE_RUNTIME: &str = "cef";

/// Explicit overrides, read once from the App's environment. The native
/// fixture sets all three; a packaged App sets none.
#[derive(Default)]
pub(super) struct Overrides {
    pub program: Option<OsString>,
    pub guard: Option<OsString>,
    pub runtime: Option<OsString>,
}
impl Overrides {
    pub(super) fn from_env() -> Self {
        Self {
            program: std::env::var_os("TAKOSUMI_BROWSER_PROCESS_PROGRAM"),
            guard: std::env::var_os("TAKOSUMI_BROWSER_PROCESS_GUARD"),
            runtime: std::env::var_os("TAKOSUMI_BROWSER_PROCESS_RUNTIME"),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Runtime {
    /// The fixture's own directory: it must already exist.
    Given(PathBuf),
    /// A per-user directory the App creates (0700) when it is missing.
    Owned(PathBuf),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Plan {
    pub program: PathBuf,
    pub guard: PathBuf,
    pub runtime: Runtime,
}

/// Decide where the browser, its guard and the runtime root are, without
/// touching the filesystem. Each explicit override wins; otherwise the browser
/// and guard come from `<resources>/cef/` and the runtime root is
/// `$XDG_RUNTIME_DIR/takosumi-cef`, or `<app cache>/cef` only when
/// `XDG_RUNTIME_DIR` is unset (an empty value counts as unset). A relative
/// runtime or cache directory is refused rather than resolved against the
/// working directory.
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
    let runtime = match (
        &overrides.runtime,
        xdg_runtime.filter(|dir| !dir.is_empty()),
    ) {
        (Some(path), _) => Runtime::Given(PathBuf::from(path)),
        (None, Some(dir)) if Path::new(dir).is_absolute() => {
            Runtime::Owned(Path::new(dir).join(RUNTIME))
        }
        (None, Some(_)) => {
            return Err(Error::InvalidRequest(
                "XDG_RUNTIME_DIR must be an absolute path.",
            ))
        }
        (None, None) => match cache {
            Some(cache) if cache.is_absolute() => Runtime::Owned(cache.join(CACHE_RUNTIME)),
            _ => {
                return Err(Error::InvalidRequest(
                    "No private runtime directory is available for the browser.",
                ))
            }
        },
    };
    Ok(Plan {
        program,
        guard,
        runtime,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Located {
    pub program: PathBuf,
    pub guard: PathBuf,
    pub runtime: PathBuf,
}

/// Check the planned files and directories: both executables must resolve to
/// existing files, and the runtime root must be private to `euid` (an owned
/// one is created first). Every returned path is canonical.
pub(super) fn locate(plan: Plan, euid: u32) -> Result<Located> {
    let program = existing_file(&plan.program, "Browser process executable is unavailable.")?;
    let guard = existing_file(&plan.guard, "Browser guard is unavailable.")?;
    let runtime = match plan.runtime {
        Runtime::Given(path) => private_runtime(&path, euid)?,
        Runtime::Owned(path) => {
            private_directory(&path)?;
            private_runtime(&path, euid)?
        }
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
        .map_err(|error| failure(format!("{missing} ({}: {error})", path.display())))?;
    if !canonical.is_file() {
        return Err(Error::InvalidRequest(missing));
    }
    Ok(canonical)
}

/// The runtime root holds each browser's sockets and temporary files: a plain
/// directory (not a link) owned by `euid`, closed to group and others, and
/// short enough that Chromium's singleton socket fits `sockaddr_un` for every
/// runtime name this App instance could plausibly use.
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
    if !socket_path_fits(&canonical.join(format!("p-{}-999999", std::process::id()))) {
        return Err(Error::InvalidRequest(
            "Browser runtime directory path is too long.",
        ));
    }
    Ok(canonical)
}

/// Chromium's temporary singleton socket under a browser's runtime directory
/// must fit `sockaddr_un` (108 bytes including the terminator).
pub(super) fn socket_path_fits(runtime: &Path) -> bool {
    runtime
        .join("tmp/org.chromium.Chromium.XXXXXX/SingletonSocket")
        .as_os_str()
        .len()
        < 108
}

/// Chromium refuses its sandbox to root, so a root App (the native fixture
/// runs as root) starts browsers with `--no-sandbox`. Otherwise only an
/// explicit `TAKOSUMI_BROWSER_PROCESS_NO_SANDBOX=1` opts out. The browser
/// reads the same decision from its arguments.
pub(super) fn without_sandbox(euid: u32, opt_out: Option<&OsStr>) -> bool {
    euid == 0 || opt_out == Some(OsStr::new("1"))
}

#[cfg(test)]
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

    #[test]
    fn a_packaged_app_finds_its_browser_in_resources_and_sockets_in_the_runtime_dir() {
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
                runtime: Runtime::Owned("/run/user/1000/takosumi-cef".into()),
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
            Runtime::Owned("/run/user/7/takosumi-cef".into())
        );
    }

    #[test]
    fn the_cache_runtime_is_used_only_without_xdg_runtime_dir() {
        let cache = Some(Path::new("/home/a/.cache/com.takosumi"));
        for unset in [None, Some(OsStr::new(""))] {
            let chosen = plan(&Overrides::default(), Some(Path::new("/r")), unset, cache).unwrap();
            assert_eq!(
                chosen.runtime,
                Runtime::Owned("/home/a/.cache/com.takosumi/cef".into())
            );
        }
        assert!(plan(
            &Overrides::default(),
            Some(Path::new("/r")),
            Some(OsStr::new("relative/run")),
            cache
        )
        .is_err());
        for cache in [None, Some(Path::new("relative/cache"))] {
            assert!(plan(&Overrides::default(), Some(Path::new("/r")), None, cache).is_err());
        }
    }

    #[test]
    fn without_resources_or_overrides_there_is_no_browser() {
        let run = Some(OsStr::new("/run/user/1000"));
        assert!(plan(&Overrides::default(), None, run, None).is_err());
        assert!(plan(&overrides(Some("/b"), None, None), None, run, None).is_err());
        assert!(plan(&overrides(None, Some("/g"), None), None, run, None).is_err());
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
        let mode = std::fs::metadata(root.join("x").join(RUNTIME))
            .unwrap()
            .permissions()
            .mode();
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
        assert_eq!(mode & 0o777, 0o700);
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
        private_directory(&root.join("ok")).unwrap();
        let canonical = root.join("ok").canonicalize().unwrap();
        let ok = attempt(Runtime::Given(root.join("ok")), euid());
        // Another user's directory, by the check's own identity input.
        let foreign = attempt(Runtime::Given(root.join("ok")), euid().wrapping_add(1));
        // A fixture runtime is never created on its behalf.
        let absent = attempt(Runtime::Given(root.join("absent")), euid());
        private_directory(&root.join("open")).unwrap();
        std::fs::set_permissions(root.join("open"), std::fs::Permissions::from_mode(0o750))
            .unwrap();
        let open = attempt(Runtime::Owned(root.join("open")), euid());
        std::os::unix::fs::symlink(root.join("ok"), root.join("link")).unwrap();
        let link = attempt(Runtime::Owned(root.join("link")), euid());
        let long = root.join("l".repeat(60));
        let too_long = attempt(Runtime::Owned(long.clone()), euid());
        let long_created = long.is_dir();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(ok.unwrap().runtime, canonical);
        assert!(foreign.is_err());
        assert!(absent.is_err());
        assert!(open.is_err());
        assert!(link.is_err());
        assert!(too_long.is_err());
        assert!(
            long_created,
            "the owned directory is created before its checks"
        );
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
}
