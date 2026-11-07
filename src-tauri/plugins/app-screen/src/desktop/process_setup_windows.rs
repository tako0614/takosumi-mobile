//! Windows checks of the browser plan (see `process_setup.rs`): the packaged
//! `cef\` executables, the runtime root `<app cache>\cef` (Tauri's app cache
//! directory is `%LOCALAPPDATA%\<identifier>` on Windows) and the runtime
//! directory locks. No `XDG_RUNTIME_DIR` exists there, so the plan's only
//! candidate is the cache.
//!
//! Every path the App gives the browser, and every one it compares with the
//! browser's replies, is canonical in the ordinary spelling (`C:\…`,
//! `\\server\share\…`): `canonicalize` resolves links, short (8.3) names and
//! letter case and adds a verbatim `\\?\` prefix, which is then dropped, as
//! the browser's own final-path check does.
//!
//! An executable from the App's bundle (`<resources>\cef`, where an installed
//! App's resources are its installation directory) must be a plain file (no
//! reparse point) directly in that directory, itself a plain directory, so a
//! link or junction cannot make anything outside the installation stand in
//! for the browser or the guard. A fixture override only has to resolve to a
//! file.
//!
//! A runtime root must be a plain directory (no reparse point: not a link, a
//! junction or another reparse point) whose canonical path lies inside this
//! user's profile directory, whose default access control limits it to the
//! user, SYSTEM and administrators. Not checked: the directory's owner SID or
//! its access control list (an inherited or explicit grant to others is not
//! detected), and whether the profile itself is on a network share.
use super::{
    existing_file, failure, private_directory, Candidate, Located, Plan, Runtime, CACHE_RUNTIME,
    RUNTIME_LOCK,
};
use crate::{Error, Result};
use std::{
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

const FILE_SHARE_DELETE: u32 = 0x4;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const ERROR_SHARING_VIOLATION: i32 = 32;

/// The user a runtime root must belong to: their profile directory
/// (canonical, ordinary spelling).
pub(in crate::desktop) struct Identity {
    profile: PathBuf,
}

impl Identity {
    /// From the user's home (profile) directory, as Tauri resolves it.
    pub(in crate::desktop) fn of(home: &Path) -> Result<Self> {
        Ok(Self {
            profile: canonical(home)?,
        })
    }
}

/// `canonicalize`'s result without its verbatim prefix: `\\?\C:\…` becomes
/// `C:\…` and `\\?\UNC\server\share\…` becomes `\\server\share\…`. Anything
/// else (a non-UTF-8 path, a device path) is returned unchanged; the browser
/// refuses such a directory.
pub(super) fn spelled(verbatim: PathBuf) -> PathBuf {
    let Some(text) = verbatim.to_str() else {
        return verbatim;
    };
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{share}"));
    }
    if let Some(disk) = text
        .strip_prefix(r"\\?\")
        .filter(|rest| rest.as_bytes().get(1) == Some(&b':'))
    {
        return PathBuf::from(disk);
    }
    verbatim
}

pub(in crate::desktop) fn canonical(path: &Path) -> Result<PathBuf> {
    path.canonicalize().map(spelled).map_err(failure)
}

/// A directory itself, not a link, junction or other reparse point.
fn plain_directory(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_dir() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
}

/// As on Linux: both executables must resolve to existing files, and the
/// runtime root must qualify (a fixture's must already exist; otherwise the
/// cache candidate is created when missing). Every returned path is canonical
/// in the ordinary spelling.
pub(in crate::desktop) fn locate(plan: Plan, identity: &Identity) -> Result<Located> {
    let bundle = plan.bundle.as_deref();
    let program = executable(
        &plan.program,
        bundle,
        "Browser process executable is unavailable.",
    )?;
    let guard = executable(&plan.guard, bundle, "Browser guard is unavailable.")?;
    let bundled = |path: &Path| bundle.is_some_and(|bundle| path.parent() == Some(bundle));
    let bundle = match bundle {
        Some(bundle) if bundled(&plan.program) || bundled(&plan.guard) => Some(canonical(bundle)?),
        _ => None,
    };
    let runtime = match plan.runtime {
        Runtime::Given(path) => private_runtime(&path, identity)?,
        Runtime::Search(candidates) => search(&candidates, identity)?,
    };
    Ok(Located {
        program,
        guard,
        runtime,
        bundle,
    })
}

/// Before each spawn: every executable located in the bundle directory must
/// still be a plain file directly in that plain directory, at the same
/// canonical path. Links, junctions, moves and deletions since the App
/// started are refused; an in-place overwrite of a plain file (or a file
/// moved back in its place) is not detected, since nothing holds the files
/// open, and the moment between this check and the process creation
/// remains.
pub(in crate::desktop) fn recheck(
    bundle: Option<&Path>,
    program: &Path,
    guard: &Path,
) -> Result<()> {
    let Some(bundle) = bundle else {
        return Ok(());
    };
    for (path, missing) in [
        (program, "Browser process executable is unavailable."),
        (guard, "Browser guard is unavailable."),
    ] {
        if path.parent() == Some(bundle) && bundled_file(bundle, path, missing)? != path {
            return Err(Error::InvalidRequest(
                "A bundled browser executable changed after the App started.",
            ));
        }
    }
    Ok(())
}

/// An executable of the plan: one from the App's bundle must be a plain file
/// directly in the plain bundle directory (`bundled_file`); a fixture
/// override only has to resolve to a file.
fn executable(path: &Path, bundle: Option<&Path>, missing: &'static str) -> Result<PathBuf> {
    match bundle {
        Some(bundle) if path.parent() == Some(bundle) => bundled_file(bundle, path, missing),
        _ => existing_file(path, missing),
    }
}

/// `path` in the bundle directory `bundle`: the directory must be a plain
/// directory and the file a plain file (neither a link, a junction nor
/// another reparse point), and the file's canonical path must lie directly
/// in the directory's. Returns the canonical file in the ordinary spelling.
fn bundled_file(bundle: &Path, path: &Path, missing: &'static str) -> Result<PathBuf> {
    let unavailable = |at: &Path, error: std::io::Error| {
        failure(format!("{missing} ({}: {error})", at.display()))
    };
    let directory = bundle
        .symlink_metadata()
        .map_err(|error| unavailable(bundle, error))?;
    if !plain_directory(&directory) {
        return Err(Error::InvalidRequest(
            "The App's bundled browser directory must be a plain directory in its installation.",
        ));
    }
    let file = path
        .symlink_metadata()
        .map_err(|error| unavailable(path, error))?;
    if !file.file_type().is_file() || file.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(Error::InvalidRequest(
            "A bundled browser executable must be a plain file in the App's installation.",
        ));
    }
    let resolved = existing_file(path, missing)?;
    if resolved.parent() != Some(canonical(bundle)?.as_path()) {
        return Err(Error::InvalidRequest(
            "A bundled browser executable must lie in the App's installation.",
        ));
    }
    Ok(resolved)
}

fn search(candidates: &[Candidate], identity: &Identity) -> Result<PathBuf> {
    let mut reasons = Vec::new();
    for candidate in candidates {
        let (Candidate::Xdg(place) | Candidate::Cache(place)) = candidate;
        let root = match candidate {
            // Never planned on Windows (the App passes no XDG directory).
            Candidate::Xdg(_) => {
                reasons.push(format!(
                    "{}: XDG_RUNTIME_DIR is not used on Windows",
                    place.display()
                ));
                continue;
            }
            Candidate::Cache(cache) => cache.join(CACHE_RUNTIME),
        };
        let usable = match root.symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                private_directory(&root).and_then(|()| private_runtime(&root, identity))
            }
            Err(error) => Err(failure(error)),
            Ok(_) => private_runtime(&root, identity),
        };
        match usable {
            Ok(root) => return Ok(root),
            Err(error) => reasons.push(format!("{}: {error}", place.display())),
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

/// A plain directory inside this user's profile (see the module comment for
/// what is not checked). Returns its canonical ordinary spelling.
fn private_runtime(path: &Path, identity: &Identity) -> Result<PathBuf> {
    let metadata = path.symlink_metadata().map_err(failure)?;
    if !plain_directory(&metadata) {
        return Err(Error::InvalidRequest(
            "Browser runtime directory must be a plain directory.",
        ));
    }
    let canonical = canonical(path)?;
    if !canonical.starts_with(&identity.profile) {
        return Err(Error::InvalidRequest(
            "Browser runtime directory must be inside this user's profile.",
        ));
    }
    Ok(canonical)
}

/// Create a fresh runtime directory's lock file and hold it open with no
/// read or write sharing: while this handle is open, every other open of the
/// file fails with a sharing violation, which is how another App instance
/// tells it is held (Windows' counterpart of the Linux `flock`). Delete
/// sharing lets the App remove the directory while it still holds the lock,
/// as on Linux. Not inherited by the guard (std's handles are not
/// inheritable), and never opened through a link.
pub(in crate::desktop) fn lock_runtime(runtime: &Path) -> Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(runtime.join(RUNTIME_LOCK))
        .map_err(|error| {
            if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION) {
                Error::InvalidRequest(
                    "Browser runtime directory is locked by another App instance.",
                )
            } else {
                failure(error)
            }
        })
}

/// The lock of an existing runtime directory, if this App can take it now:
/// `None` while another instance holds it (a sharing violation), and when
/// there is no lock file. A lock file that is a link or other reparse point
/// is an error, never followed or judged.
pub(super) fn stale_lock(runtime: &Path) -> Result<Option<std::fs::File>> {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(runtime.join(RUNTIME_LOCK))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION) => return Ok(None),
        Err(error) => return Err(failure(error)),
    };
    let metadata = file.metadata().map_err(failure)?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(Error::InvalidRequest(
            "Browser runtime lock file is a link.",
        ));
    }
    Ok(Some(file))
}

/// As on Linux, inside the verified runtime root: only plain directories
/// named `p-<pid>-<n>` whose lock no App instance holds are removed. The
/// root lies inside this user's profile, so its entries are the user's.
pub(in crate::desktop) fn sweep_runtime(root: &Path, _identity: &Identity) -> Vec<String> {
    super::sweep(root, plain_directory)
}

#[cfg(test)]
mod windows_tests {
    use super::super::{
        claim_runtime, plan, remove_runtime, runtime_name, Overrides, BROWSER, GUARD, PACKAGED,
    };
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("sumi-ps-{name}-{}", std::process::id()));
        private_directory(&root).unwrap();
        canonical(&root).unwrap()
    }

    #[test]
    fn a_packaged_windows_app_finds_exe_files_and_searches_only_its_cache() {
        let planned = plan(
            &Overrides::default(),
            Some(Path::new(r"C:\Program Files\Takosumi")),
            None,
            Some(Path::new(r"C:\Users\a\AppData\Local\com.takosumi")),
        )
        .unwrap();
        assert_eq!(
            planned,
            Plan {
                program: r"C:\Program Files\Takosumi\cef\takosumi-cef-browser-process.exe".into(),
                guard: r"C:\Program Files\Takosumi\cef\takosumi-browser-guard.exe".into(),
                runtime: Runtime::Search(vec![Candidate::Cache(
                    r"C:\Users\a\AppData\Local\com.takosumi".into()
                )]),
                bundle: Some(r"C:\Program Files\Takosumi\cef".into()),
            }
        );
    }

    #[test]
    fn bundled_executables_must_be_plain_files_in_the_installation() {
        let root = scratch("bundle");
        let identity = Identity {
            profile: root.clone(),
        };
        let cache = root.join("cache");
        private_directory(&cache).unwrap();
        let installed = |name: &str| {
            let install = root.join(name);
            private_directory(&install.join(PACKAGED)).unwrap();
            for file in [BROWSER, GUARD] {
                std::fs::write(install.join(PACKAGED).join(file), b"").unwrap();
            }
            install
        };
        let resolve = |overrides: &Overrides, install: &Path| {
            locate(
                plan(overrides, Some(install), None, Some(&cache)).unwrap(),
                &identity,
            )
        };
        // `mklink /J` makes a junction; with no switch, a file symbolic link.
        let mklink = |kind: Option<&str>, link: &Path, target: &Path| {
            let made = std::process::Command::new("cmd")
                .args(["/d", "/c", "mklink"])
                .args(kind)
                .arg(link)
                .arg(target)
                .output()
                .unwrap();
            made.status.success()
        };
        // The installed App resolves both from its own bundle.
        let install = installed("install");
        let found = resolve(&Overrides::default(), &install);
        // Before each spawn the same checks run again: they pass while the
        // installation is unchanged and fail once a bundled file is gone.
        let recheck_found = || {
            found.as_ref().ok().map(|found| {
                recheck(found.bundle.as_deref(), &found.program, &found.guard)
                    .map_err(|error| error.to_string())
            })
        };
        let rechecked = recheck_found();
        let guard_file = install.join(PACKAGED).join(GUARD);
        let renamed = install.join(PACKAGED).join("moved.exe");
        std::fs::rename(&guard_file, &renamed).unwrap();
        let after_change = recheck_found();
        std::fs::rename(&renamed, &guard_file).unwrap();
        // A bundle directory that is a junction to files elsewhere (no
        // privilege needed) is refused, even though the files exist.
        let elsewhere = installed("elsewhere");
        let joined = root.join("joined");
        private_directory(&joined).unwrap();
        assert!(mklink(
            Some("/J"),
            &joined.join(PACKAGED),
            &elsewhere.join(PACKAGED)
        ));
        let through_junction = resolve(&Overrides::default(), &joined);
        // A missing bundled guard is refused with its reason.
        let partial = installed("partial");
        std::fs::remove_file(partial.join(PACKAGED).join(GUARD)).unwrap();
        let missing = resolve(&Overrides::default(), &partial);
        // A fixture override may point anywhere; the executable left to the
        // bundle is still checked.
        let outside = elsewhere.join(PACKAGED).join(BROWSER);
        let mixed = Overrides {
            program: Some(outside.clone().into()),
            ..Overrides::default()
        };
        let mixed_found = resolve(&mixed, &install);
        let mixed_missing = resolve(&mixed, &partial);
        // A file symbolic link in the bundle (when this account may create
        // one) is refused too.
        let linked = installed("linked");
        let browser = linked.join(PACKAGED).join(BROWSER);
        std::fs::remove_file(&browser).unwrap();
        let symlink =
            mklink(None, &browser, &outside).then(|| resolve(&Overrides::default(), &linked));
        // The junction is removed, never followed.
        std::fs::remove_dir(joined.join(PACKAGED)).unwrap();
        let kept = elsewhere.join(PACKAGED).join(GUARD).is_file();
        std::fs::remove_dir_all(&root).unwrap();
        let found = found.unwrap();
        assert_eq!(found.program, install.join(PACKAGED).join(BROWSER));
        assert_eq!(found.guard, install.join(PACKAGED).join(GUARD));
        assert_eq!(found.bundle, Some(install.join(PACKAGED)));
        rechecked.unwrap().unwrap();
        let reason = after_change.unwrap().unwrap_err();
        assert!(reason.contains("Browser guard is unavailable"), "{reason}");
        let reason = through_junction.unwrap_err().to_string();
        assert!(reason.contains("plain directory"), "{reason}");
        let reason = missing.unwrap_err().to_string();
        assert!(reason.contains("Browser guard is unavailable"), "{reason}");
        let mixed_found = mixed_found.unwrap();
        assert_eq!(mixed_found.program, outside);
        assert_eq!(mixed_found.guard, install.join(PACKAGED).join(GUARD));
        assert_eq!(mixed_found.bundle, Some(install.join(PACKAGED)));
        assert!(mixed_missing.is_err());
        if let Some(symlink) = symlink {
            let reason = symlink.unwrap_err().to_string();
            assert!(reason.contains("plain file"), "{reason}");
        }
        assert!(kept);
    }

    #[test]
    fn the_verbatim_prefix_is_dropped_and_nothing_else_changes() {
        for (verbatim, ordinary) in [
            (r"\\?\C:\Users\a\x", r"C:\Users\a\x"),
            (r"\\?\UNC\server\share\x", r"\\server\share\x"),
            (r"\\?\Volume{0}\x", r"\\?\Volume{0}\x"),
            (r"C:\already", r"C:\already"),
        ] {
            assert_eq!(spelled(PathBuf::from(verbatim)), PathBuf::from(ordinary));
        }
        let here = canonical(&std::env::temp_dir()).unwrap();
        assert!(!here.to_str().unwrap().starts_with(r"\\?\"), "{here:?}");
        assert!(here.is_absolute());
    }

    #[test]
    fn a_runtime_root_must_be_a_plain_directory_inside_the_profile() {
        let root = scratch("root");
        let identity = Identity {
            profile: root.join("profile"),
        };
        private_directory(&identity.profile.join("ok")).unwrap();
        private_directory(&root.join("outside")).unwrap();
        // A junction (no privilege needed) inside the profile to inside it.
        let made = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(identity.profile.join("junction"))
            .arg(identity.profile.join("ok"))
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        let ok = private_runtime(&identity.profile.join("ok"), &identity);
        let outside = private_runtime(&root.join("outside"), &identity);
        let junction = private_runtime(&identity.profile.join("junction"), &identity);
        let missing = private_runtime(&identity.profile.join("absent"), &identity);
        // The junction itself is removed, never followed.
        std::fs::remove_dir(identity.profile.join("junction")).unwrap();
        let kept = identity.profile.join("ok").is_dir();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(ok.unwrap(), identity.profile.join("ok"));
        assert!(outside.is_err());
        assert!(junction.is_err());
        assert!(missing.is_err());
        assert!(kept);
    }

    #[test]
    fn a_missing_cache_runtime_root_is_created_and_located() {
        let root = scratch("cache");
        let identity = Identity {
            profile: root.clone(),
        };
        let cache = root.join("app-cache");
        private_directory(&cache).unwrap();
        std::fs::write(root.join("program.exe"), b"").unwrap();
        let absent = !cache.join(CACHE_RUNTIME).exists();
        let located = locate(
            Plan {
                program: root.join("program.exe"),
                guard: root.join("program.exe"),
                runtime: Runtime::Search(vec![
                    Candidate::Xdg(root.join("never-used")),
                    Candidate::Cache(cache.clone()),
                ]),
                bundle: None,
            },
            &identity,
        );
        let created = cache.join(CACHE_RUNTIME).is_dir();
        // Outside the profile, the created root is refused with the reason.
        let elsewhere = Identity {
            profile: root.join("other-profile"),
        };
        let refused = search(&[Candidate::Cache(root.join("other-cache"))], &elsewhere);
        std::fs::remove_dir_all(&root).unwrap();
        assert!(absent, "the root did not exist before");
        let located = located.unwrap();
        assert_eq!(located.runtime, cache.join(CACHE_RUNTIME));
        assert_eq!(located.program, root.join("program.exe"));
        assert!(created);
        let reason = refused.unwrap_err().to_string();
        assert!(reason.contains("inside this user's profile"), "{reason}");
    }

    #[test]
    fn a_held_runtime_lock_is_never_judged_stale_and_its_directory_can_go() {
        let root = scratch("lock");
        let held_dir = root.join(runtime_name(4_000_001, 1));
        private_directory(&held_dir).unwrap();
        let held = lock_runtime(&held_dir).unwrap();
        let judged_held = stale_lock(&held_dir).unwrap().is_none();
        let claimed_held = claim_runtime(&held_dir).unwrap();
        let second = lock_runtime(&held_dir).is_err();
        // The App removes an ended browser's directory while holding its lock.
        remove_runtime(&held_dir).unwrap();
        let removed_while_held = !held_dir.exists();
        drop(held);
        // A lock file nobody holds is stale: the directory is replaced.
        let stale_dir = root.join(runtime_name(4_000_001, 2));
        private_directory(&stale_dir.join("tmp")).unwrap();
        drop(lock_runtime(&stale_dir).unwrap());
        let claimed_stale = claim_runtime(&stale_dir).unwrap();
        let stale_gone = !stale_dir.exists();
        // No lock file: never judged.
        let unjudged = root.join(runtime_name(4_000_001, 3));
        private_directory(&unjudged).unwrap();
        let claimed_unjudged = claim_runtime(&unjudged).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        assert!(judged_held && !claimed_held && second);
        assert!(
            removed_while_held,
            "a held lock does not keep its directory"
        );
        assert!(claimed_stale && stale_gone);
        assert!(!claimed_unjudged);
    }

    #[test]
    fn the_sweep_removes_only_unlocked_runtime_directories() {
        let root = scratch("sweep");
        let identity = Identity {
            profile: root.clone(),
        };
        let stale = root.join(runtime_name(4_000_002, 1));
        private_directory(&stale.join("tmp")).unwrap();
        drop(lock_runtime(&stale).unwrap());
        let live = root.join(runtime_name(4_000_002, 2));
        private_directory(&live).unwrap();
        let held = lock_runtime(&live).unwrap();
        let other = root.join("keep");
        private_directory(&other).unwrap();
        drop(lock_runtime(&other).unwrap());
        let problems = sweep_runtime(&root, &identity);
        drop(held);
        let mut left = std::fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        left.sort();
        std::fs::remove_dir_all(&root).unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(left, vec!["keep".to_owned(), runtime_name(4_000_002, 2)]);
    }
}
