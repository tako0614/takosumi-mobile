//! Explicit experiment behind the existing shell-only `open` command, on
//! Linux and (an unsandboxed test candidate) Windows. The OS-specific pieces
//! are small cfg'd functions here and in `process_windows.rs`.
#[cfg(windows)]
use super::process_windows as windows;
use super::{failure, private_directory, process_setup as setup};
#[cfg(target_os = "linux")]
use crate::browser_process::write_until;
use crate::{
    browser_process::{external_link, read_packet, Reply, Request, MAX_PACKET_BYTES},
    Error, OpenRequest, Result,
};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{BufReader, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicI32, AtomicU64, AtomicU8, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};
#[cfg(target_os = "linux")]
use std::{
    os::fd::AsRawFd,
    os::unix::{fs::OpenOptionsExt, process::CommandExt},
    process::ChildStdin,
};
use tauri::{AppHandle, Manager};

#[derive(Clone)]
pub(super) struct Backend(Arc<Mutex<State>>, Arc<ExitPhase>);
#[derive(Default)]
struct ExitPhase {
    // 0: running, 1: closing, 2: all owned browser guards finished normally.
    state: AtomicU8,
    code: AtomicI32,
    requests: AtomicU64,
}
struct State {
    program: PathBuf,
    guard: PathBuf,
    /// Windows: the bundle directory the executables were located in, if
    /// any; re-checked before each spawn (`setup::recheck`).
    #[cfg(windows)]
    bundle: Option<PathBuf>,
    directory: PathBuf,
    runtime_root: PathBuf,
    /// Root, or the fixture's explicit opt-out: see `setup::without_sandbox`.
    /// Always false on Windows, where the browser decides its sandbox itself.
    no_sandbox: bool,
    processes: BTreeMap<String, Process>,
    spawner: mpsc::Sender<SpawnRequest>,
}
type SpawnRequest = (Command, mpsc::SyncSender<std::io::Result<Child>>);
/// The App's end of a browser's control pipe: non-blocking on Linux, a
/// bounded writer thread on Windows (`process_windows::Input`).
#[cfg(target_os = "linux")]
type Input = ChildStdin;
#[cfg(windows)]
type Input = windows::Input;

/// Linux delivers a child's parent-death signal when the *thread* that forked
/// it exits, not only when the App process exits. Tauri's blocking pool
/// retires idle threads, which would SIGTERM every guard forked from one. Fork
/// guards only from this thread, which stays alive for the whole App process.
/// If it ever panicked, its guards drain their browsers and later opens fail
/// as unavailable: closed, never an unowned browser. Requesters always wait
/// for the reply, so a spawned guard is never left untracked.
#[cfg(target_os = "linux")]
fn spawner() -> Result<mpsc::Sender<SpawnRequest>> {
    let (send, requests) = mpsc::channel::<SpawnRequest>();
    std::thread::Builder::new()
        .name("browser-process-parent".into())
        .spawn(move || {
            for (mut command, reply) in requests {
                let _ = reply.send(command.spawn());
            }
            // Never end the parent-death link before the App process exits.
            loop {
                std::thread::park();
            }
        })
        .map_err(failure)?;
    Ok(send)
}
/// Windows has no parent-death signal. Every guard starts inside one Job
/// Object this thread owns: created suspended, assigned, then resumed, so it
/// runs no code outside the job, and detached from any console (no console
/// window and no console host process; its standard handles are the pipes
/// and log passed explicitly). The job ends
/// every guard and browser in it when its last handle closes: when the App
/// process ends, however it ends, or when every requester is gone (the App
/// state was dropped, or a test ended). Spawning from one thread also keeps
/// one guard's pipe ends out of another's inherited handles.
#[cfg(windows)]
fn spawner() -> Result<mpsc::Sender<SpawnRequest>> {
    use crate::windows_job::{spawn_in, Job, DETACHED_PROCESS};
    let job = Job::kill_on_close().map_err(failure)?;
    let (send, requests) = mpsc::channel::<SpawnRequest>();
    std::thread::Builder::new()
        .name("browser-process-parent".into())
        .spawn(move || {
            for (mut command, reply) in requests {
                let _ = reply.send(spawn_in(&job, &mut command, DETACHED_PROCESS));
            }
            drop(job);
        })
        .map_err(failure)?;
    Ok(send)
}
static NEXT_RUNTIME: AtomicU64 = AtomicU64::new(1);
/// Why waiting for a reply ended without one: a timeout leaves a late answer
/// queued, a closed channel means the browser is ending, a fault is an error.
enum Wait {
    TimedOut,
    Closed,
    /// The browser answered this request with an error: nothing is pending.
    Rejected(String),
    Fault(Error),
}
impl From<Wait> for Error {
    fn from(wait: Wait) -> Self {
        match wait {
            Wait::TimedOut => Error::InvalidRequest("Browser response timed out."),
            Wait::Closed => Error::InvalidRequest("Browser replies closed."),
            Wait::Rejected(message) => failure(message),
            Wait::Fault(error) => error,
        }
    }
}
struct Process {
    child: Child,
    input: Option<Input>,
    replies: mpsc::Receiver<std::result::Result<Reply, String>>,
    profile: String,
    directory: PathBuf,
    next: u64,
    uncertain: bool,
    /// The key of the open being confirmed, so a late creation reply can still
    /// be verified after a timeout.
    opening: Option<String>,
    opened: serde_json::Value,
    guard_receipt: PathBuf,
    /// This browser's `p-<pid>-<n>` directory; removed once its guard ended
    /// (kept in fixture builds, see `setup::KEEP_ENDED_RUNTIME`).
    runtime: PathBuf,
    /// The exclusive lock on the runtime directory's lock file, released only
    /// after `Drop` has removed the directory (fields drop after `drop`).
    _runtime_lock: std::fs::File,
    program: PathBuf,
    /// Opening order: an App quit closes the oldest profile first.
    order: u64,
    /// The guard's stop request (Windows): this App created it under a fresh
    /// name the guard opened at start; setting it makes the guard drain.
    #[cfg(windows)]
    stop: crate::windows_job::Event,
    /// Whether Windows accepted the foreground grant before the last reopen
    /// (`None` before any reopen); reported by `status`.
    #[cfg(windows)]
    foreground_grant: Option<bool>,
}
static NEXT_ORDER: AtomicU64 = AtomicU64::new(1);
/// How long a browser that agreed to stop (it answered Stopped, or its pipe
/// closed after its last window) may take to end normally. It may still be
/// flushing its profile; on a stalled disk that exceeded a few seconds (pk1),
/// and an App that cannot finish quitting is worse than waiting longer.
const STOP_FINISH: Duration = Duration::from_secs(30);
/// How long writing one request to a browser's control pipe may take.
const WRITE_BOUND: Duration = Duration::from_secs(20);
/// A request that was not written, and whether the browser had already closed
/// its end of the pipe (it is ending) rather than, say, not reading in time.
struct Unsent {
    error: Error,
    closed: bool,
}
/// Openers started and not yet ended; a burst beyond this is dropped.
#[cfg(not(test))]
static OPENERS: AtomicU64 = AtomicU64::new(0);
/// Hand a validated link to the desktop's opener without waiting for it. Its
/// own process group keeps the desktop browser it may start out of the App's
/// terminal signals.
#[cfg(all(not(test), target_os = "linux"))]
fn open_externally(url: &str) {
    if OPENERS.fetch_add(1, Ordering::SeqCst) >= 8 {
        OPENERS.fetch_sub(1, Ordering::SeqCst);
        eprintln!("Too many external links are opening; this one was dropped.");
        return;
    }
    let url = url.to_owned();
    // The thread that starts the opener also reaps it, so no child is ever
    // left unwatched and every slot is returned.
    let started = std::thread::Builder::new()
        .name("external-open".into())
        .spawn(move || {
            let mut command = Command::new("xdg-open");
            command
                .arg(&url)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0);
            match command.spawn() {
                Ok(mut child) => {
                    let _ = child.wait();
                }
                Err(error) => eprintln!("The external link could not be opened: {error}"),
            }
            OPENERS.fetch_sub(1, Ordering::SeqCst);
        });
    if let Err(error) = started {
        OPENERS.fetch_sub(1, Ordering::SeqCst);
        eprintln!("The external link could not be opened: {error}");
    }
}
/// Windows: the person's default browser through the shell's `open`, on a
/// thread of its own, within the same budget of openers.
#[cfg(all(not(test), windows))]
fn open_externally(url: &str) {
    if OPENERS.fetch_add(1, Ordering::SeqCst) >= 8 {
        OPENERS.fetch_sub(1, Ordering::SeqCst);
        eprintln!("Too many external links are opening; this one was dropped.");
        return;
    }
    let url = url.to_owned();
    let started = std::thread::Builder::new()
        .name("external-open".into())
        .spawn(move || {
            if !windows::shell_open(&url) {
                eprintln!("The external link could not be opened.");
            }
            OPENERS.fetch_sub(1, Ordering::SeqCst);
        });
    if let Err(error) = started {
        OPENERS.fetch_sub(1, Ordering::SeqCst);
        eprintln!("The external link could not be opened: {error}");
    }
}
/// Tests never start the desktop's opener: they record what would be opened.
#[cfg(test)]
static OPENED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
#[cfg(test)]
fn open_externally(url: &str) {
    OPENED.lock().unwrap().push(url.to_owned());
}

/// Why this App's desktop browser cannot open connections. The home and the
/// rest of the App keep running; `open` and `clear_profile` return the reason.
pub(super) struct Unavailable(String);
pub(super) fn unavailable<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<String> {
    app.try_state::<Unavailable>()
        .map(|reason| reason.inner().0.clone())
}

/// Find the browser, its guard and the runtime root: the fixture's explicit
/// environment overrides, otherwise the App's packaged `cef/` resources and
/// the per-user runtime directory (see `process_setup`). A failure never stops
/// the App: the backend is registered as unavailable with its reason.
pub(super) fn install<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<()> {
    match prepare(app) {
        Ok(state) => {
            app.manage(Backend(
                Arc::new(Mutex::new(state)),
                Arc::new(ExitPhase::default()),
            ));
        }
        Err(error) => {
            let reason = format!("The desktop browser is unavailable: {error}");
            eprintln!("{reason}");
            app.manage(Unavailable(reason));
        }
    }
    Ok(())
}

fn prepare<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<State> {
    #[cfg(target_os = "linux")]
    let owner = {
        // SAFETY: these identity calls have no preconditions and cannot fail.
        let (uid, euid, gid, egid) = unsafe {
            (
                libc::getuid(),
                libc::geteuid(),
                libc::getgid(),
                libc::getegid(),
            )
        };
        setup::plain_identity(uid, euid, gid, egid)?;
        euid
    };
    let paths = app.path();
    // Windows: the runtime root must lie in this user's profile directory.
    #[cfg(windows)]
    let identity = setup::Identity::of(&paths.home_dir().map_err(failure)?)?;
    #[cfg(windows)]
    let owner = &identity;
    #[cfg(target_os = "linux")]
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR");
    // Windows has no XDG runtime directory: the root is `<app cache>\cef`.
    #[cfg(windows)]
    let runtime_dir: Option<std::ffi::OsString> = None;
    let plan = setup::plan(
        &setup::Overrides::from_lookup(&setup::environment),
        paths.resource_dir().ok().as_deref(),
        runtime_dir.as_deref(),
        paths.app_cache_dir().ok().as_deref(),
    )?;
    let located = setup::locate(plan, owner)?;
    let (program, guard, runtime_root) = (located.program, located.guard, located.runtime);
    #[cfg(windows)]
    let bundle = located.bundle;
    // Runtime directories of ended App instances, judged by their locks
    // (tmpfs keeps them until logout; the cache fallback across reboots).
    for problem in setup::sweep_runtime(&runtime_root, owner) {
        eprintln!("A stale browser runtime directory was not removed: {problem}");
    }
    // The sandbox opt-out is a fixture seam only; a product build runs
    // sandboxed unless it runs as root (which Chromium refuses to sandbox).
    #[cfg(target_os = "linux")]
    let no_sandbox = setup::without_sandbox(
        owner,
        setup::sandbox_opt_out(&setup::environment).as_deref(),
    );
    // The Windows browser decides its sandbox itself (off in the unsandboxed
    // test candidate); the App passes no switch.
    #[cfg(windows)]
    let no_sandbox = false;
    let directory = app
        .path()
        .app_local_data_dir()
        .map_err(failure)?
        .join("browser-process-experiment");
    private_directory(&directory)?;
    let directory = setup::canonical(&directory)?;
    Ok(State {
        program,
        guard,
        #[cfg(windows)]
        bundle,
        directory,
        runtime_root,
        no_sandbox,
        processes: BTreeMap::new(),
        spawner: spawner()?,
    })
}

pub(super) async fn open(app: AppHandle, payload: OpenRequest) -> Result<()> {
    let backend = app.state::<Backend>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        if backend.1.state.load(Ordering::Acquire) != 0 {
            return Err(Error::InvalidRequest("The browser is closing."));
        }
        let mut state = backend.0.lock().map_err(failure)?;
        if backend.1.state.load(Ordering::Acquire) != 0 {
            return Err(Error::InvalidRequest("The browser is closing."));
        }
        let profile = payload.profile.clone();
        let request = Request::Open { id: 1, payload };
        request.validate(&profile)?;
        let Request::Open { payload, .. } = request else {
            unreachable!()
        };
        let exited = state
            .processes
            .get_mut(&profile)
            .map(|process| process.child.try_wait())
            .transpose()
            .map_err(failure)?
            .flatten();
        if exited.is_some_and(|status| !status.success()) {
            return Err(Error::InvalidRequest(
                "Previous browser process ended abnormally; profile data is retained.",
            ));
        }
        if exited.is_some() {
            // A guard may exit zero after forced descendant cleanup. Reuse
            // requires its complete normal receipt, not its status alone.
            state.processes.get(&profile).unwrap().normal_guard()?;
            state.processes.remove(&profile);
        } else if let Some(process) = state.processes.get_mut(&profile) {
            if process.recover()? {
                state.processes.remove(&profile);
            }
        }
        if let Some(process) = state.processes.get_mut(&profile) {
            process.open(payload)
        } else {
            if state.processes.len() >= 32 {
                return Err(Error::InvalidRequest("Browser profile limit reached."));
            }
            let key = payload.key.clone();
            // Windows: the bundled executables are checked again now, not
            // only when the App started.
            #[cfg(windows)]
            setup::recheck(state.bundle.as_deref(), &state.program, &state.guard)?;
            let runtime = free_runtime(&state.runtime_root)?;
            let mut process = Process::spawn(
                state.program.clone(),
                state.guard.clone(),
                state.directory.join(&profile),
                runtime,
                payload,
                &state.spawner,
                state.no_sandbox,
            )?;
            // Retain ownership before waiting for the native creation reply.
            let result = process.expect_open(1, &key);
            state.processes.insert(profile, process);
            result
        }
    })
    .await
    .map_err(failure)?
}

/// The next free runtime directory for a new browser. Names are unique within
/// this App instance, but another instance sharing the root (another host on
/// an NFS cache, or another PID namespace) may have the same pid.
fn free_runtime(root: &std::path::Path) -> Result<PathBuf> {
    for _ in 0..16 {
        let candidate = root.join(setup::runtime_name(
            std::process::id(),
            NEXT_RUNTIME.fetch_add(1, Ordering::SeqCst),
        ));
        if setup::claim_runtime(&candidate)? {
            return Ok(candidate);
        }
    }
    Err(Error::InvalidRequest(
        "No free browser runtime directory name is available.",
    ))
}

fn drain(backend: &Backend) -> Result<bool> {
    let mut state = backend.0.lock().map_err(failure)?;
    // CEF cannot ask every profile's beforeunload before closing any (there is
    // no two-phase quit). Per the owner's decision (2026-11-05), close profiles
    // one at a time, oldest first: each page may confirm leaving, and a cancel
    // stops the quit with the remaining profiles still open. Profiles already
    // closed had agreed to leave; their data persists for the home to reopen.
    settle_exiting(&mut state, std::time::Instant::now())?;
    // A profile left in doubt by a late answer recovers from its late reply
    // or normal end; never close earlier profiles and then fail on one still
    // in doubt.
    for profile in state.processes.keys().cloned().collect::<Vec<_>>() {
        if state.processes.get_mut(&profile).unwrap().recover()? {
            state.processes.remove(&profile);
        }
    }
    if state.processes.values().any(|process| process.uncertain) {
        return Err(Error::InvalidRequest(
            "Previous browser command is incomplete; profile data is retained.",
        ));
    }
    let mut profiles = state
        .processes
        .iter()
        .map(|(profile, process)| (process.order, profile.clone()))
        .collect::<Vec<_>>();
    profiles.sort();
    for (_, profile) in profiles {
        let process = state.processes.get_mut(&profile).unwrap();
        match process.shutdown() {
            Ok(true) => {
                state.processes.remove(&profile);
            }
            Ok(false) => return Ok(false), // Native beforeunload cancellation.
            Err(error) => return Err(error), // Marks the browser in doubt unless it answered.
        }
    }
    Ok(true)
}
/// Remove every browser that has ended normally, rechecking until at most one
/// remains or the deadline passes. An abnormal end keeps the profile and fails.
fn settle_exiting(state: &mut State, deadline: std::time::Instant) -> Result<()> {
    loop {
        let profiles = state.processes.keys().cloned().collect::<Vec<_>>();
        for profile in profiles {
            let process = state.processes.get_mut(&profile).unwrap();
            if let Some(status) = process.child.try_wait().map_err(failure)? {
                if !status.success() {
                    process.uncertain = true;
                    return Err(Error::InvalidRequest("Browser process ended abnormally."));
                }
                process.normal_guard()?;
                state.processes.remove(&profile);
            }
        }
        if state.processes.len() <= 1 || std::time::Instant::now() >= deadline {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
pub(super) fn shutdown(app: &AppHandle) -> Result<()> {
    // An unavailable browser has nothing to close.
    let Some(backend) = app.try_state::<Backend>() else {
        return Ok(());
    };
    let backend = backend.inner().clone();
    match backend
        .1
        .state
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
    {
        Ok(_) => {}
        Err(2) => return Ok(()),
        Err(_) => return Err(Error::InvalidRequest("Browser close is already pending.")),
    }
    match drain(&backend) {
        Ok(true) => {
            backend.1.state.store(2, Ordering::Release);
            Ok(())
        }
        result => {
            backend.1.state.store(0, Ordering::Release);
            backend.1.code.store(0, Ordering::Release);
            result.and_then(|_| Err(Error::InvalidRequest("Browser close was cancelled.")))
        }
    }
}
/// Returns true while an ordinary exit must be prevented. No blocking work on
/// the UI thread, and the latch is set before waiting for any in-flight open.
pub(super) fn request_exit(app: &AppHandle, code: i32) -> bool {
    let Some(backend) = app.try_state::<Backend>() else {
        return false;
    };
    let backend = backend.inner().clone();
    backend.1.requests.fetch_add(1, Ordering::AcqRel);
    if code != 0 {
        backend.1.code.store(code, Ordering::Release);
    }
    match backend
        .1
        .state
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
    {
        Err(2) => return false,
        Err(_) => return true,
        Ok(_) => {}
    }
    {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let worker = backend.clone();
            let result = tauri::async_runtime::spawn_blocking(move || drain(&worker)).await;
            if matches!(result, Ok(Ok(true))) {
                backend.1.state.store(2, Ordering::Release);
                app.exit(backend.1.code.load(Ordering::Acquire));
            } else {
                backend.1.code.store(0, Ordering::Release);
                backend.1.state.store(0, Ordering::Release);
                if let Ok(Err(error)) = result {
                    eprintln!("Browser close remains incomplete: {error}");
                }
            }
        });
    }
    true
}
pub(super) fn terminal(app: &AppHandle) {
    let Some(backend) = app.try_state::<Backend>() else {
        return;
    };
    if backend.1.state.load(Ordering::Acquire) != 2 {
        backend.1.state.store(1, Ordering::Release);
        eprintln!("Abnormal terminal exit: browser profiles are retained.");
        if let Ok(mut state) = backend.0.try_lock() {
            state.processes.clear();
        }
    }
}

/// Clear one idle isolated profile's site data. Cookies, storage, cache and
/// site permissions live under its CEF cache root, which CEF holds open while
/// the browser runs, so the profile's browser must have ended normally first.
/// Downloads, logs, other profiles and the saved connection are kept.
pub(super) async fn clear_profile(
    app: AppHandle,
    payload: crate::ClearProfileRequest,
) -> Result<crate::ClearProfileResponse> {
    payload.validate()?;
    let backend = app.state::<Backend>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        if backend.1.state.load(Ordering::Acquire) != 0 {
            return Err(Error::InvalidRequest("The browser is closing."));
        }
        let mut state = backend.0.lock().map_err(failure)?;
        if backend.1.state.load(Ordering::Acquire) != 0 {
            return Err(Error::InvalidRequest("The browser is closing."));
        }
        release_ended(
            &mut state,
            &payload.profile,
            std::time::Instant::now() + Duration::from_secs(2),
        )?;
        // The backend lock stays held: no open of this profile can start meanwhile.
        clear_site_data(&state.directory, &payload.profile)?;
        Ok(crate::ClearProfileResponse { cleared: true })
    })
    .await
    .map_err(failure)?
}

/// Drop this profile's tracked browser once it has ended normally. One whose
/// last window was just closed is still exiting for a moment, so wait within
/// the bounded deadline before refusing it as still open; an abnormal end
/// keeps the profile and fails.
fn release_ended(state: &mut State, profile: &str, deadline: std::time::Instant) -> Result<()> {
    let Some(process) = state.processes.get_mut(profile) else {
        return Ok(());
    };
    let status = loop {
        if let Some(status) = process.child.try_wait().map_err(failure)? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            return Err(Error::InvalidRequest(STILL_OPEN));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    if !status.success() {
        return Err(Error::InvalidRequest(
            "Previous browser process ended abnormally; profile data is retained.",
        ));
    }
    process.normal_guard()?;
    state.processes.remove(profile);
    Ok(())
}

static NEXT_CLEAR: AtomicU64 = AtomicU64::new(1);
const CLEARING: &str = "cef-profiles.clearing-";
const STILL_OPEN: &str = "この接続先の画面をすべて閉じてから消去してください。";

/// CEF's own process singleton marks a live browser on this cache root, even
/// one this App instance does not track (another instance, or a browser still
/// draining after an App crash). The lock names `<hostname>-<pid>`; a lock from
/// another host, an unreadable or unparsable one, or a live PID counts as live.
/// A reused PID refuses on the safe side; reopening the profile renews the lock.
/// (Windows: Chromium's held `lockfile`, see `process_windows::singleton_live`.)
#[cfg(target_os = "linux")]
fn singleton_live(root: &std::path::Path) -> bool {
    let target = match std::fs::read_link(root.join("SingletonLock")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
        Ok(target) => target,
    };
    let Some((host, pid)) = target
        .to_str()
        .and_then(|target| target.rsplit_once('-'))
        .and_then(|(host, pid)| Some((host, pid.parse::<u32>().ok()?)))
    else {
        return true;
    };
    let local = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default();
    host != local.trim() || std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// Remove detached cache roots left by an interrupted clear. Links are never
/// followed: only plain directories are removed, with std's link-safe removal.
fn remove_detached(profile_directory: &std::path::Path) -> Result<()> {
    for entry in std::fs::read_dir(profile_directory).map_err(failure)? {
        let entry = entry.map_err(failure)?;
        if entry.file_name().to_string_lossy().starts_with(CLEARING) {
            let path = entry.path();
            let metadata = path.symlink_metadata().map_err(failure)?;
            if metadata.file_type().is_dir() {
                std::fs::remove_dir_all(&path).map_err(failure)?;
            }
        }
    }
    Ok(())
}

/// Detach the profile's CEF cache root by rename, then remove it. An
/// interrupted removal leaves only a detached directory, never a half-cleared
/// live profile; leftovers are removed by the next clear. Links are refused,
/// never followed.
fn clear_site_data(directory: &std::path::Path, profile: &str) -> Result<()> {
    fn plain_directory(path: &std::path::Path) -> Result<bool> {
        match path.symlink_metadata() {
            Ok(metadata) if metadata.file_type().is_dir() => Ok(true),
            Ok(_) => Err(Error::InvalidRequest(
                "Browser profile data is not a plain directory.",
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(failure(error)),
        }
    }
    let profile_directory = directory.join(profile);
    if !plain_directory(&profile_directory)? {
        return Ok(());
    }
    let root = profile_directory.join("cef-profiles");
    if plain_directory(&root)? {
        if singleton_live(&root) {
            return Err(Error::InvalidRequest(STILL_OPEN));
        }
        let detached = profile_directory.join(format!(
            "{CLEARING}{}-{}",
            std::process::id(),
            NEXT_CLEAR.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::rename(&root, &detached).map_err(failure)?;
    }
    remove_detached(&profile_directory)
}

pub(super) fn status(app: &AppHandle) -> Result<serde_json::Value> {
    let Some(backend) = app.try_state::<Backend>() else {
        return Err(failure(unavailable(app).unwrap_or_default()));
    };
    let state = backend.0.lock().map_err(failure)?;
    Ok(serde_json::Value::Array(state.processes.values().map(|process|{
        #[allow(unused_mut)]
        let mut entry = serde_json::json!({
            "pid":process.child.id(),"profile":process.profile,"directory":process.directory,"guardReceipt":process.guard_receipt,
            "next":process.next,"uncertain":process.uncertain,"opened":process.opened,
            "closing":backend.1.state.load(Ordering::Acquire),"exitRequests":backend.1.requests.load(Ordering::Acquire)
        });
        // Windows: whether the last reopen's foreground grant was accepted.
        #[cfg(windows)]
        {
            entry["foregroundGrant"] = serde_json::json!(process.foreground_grant);
        }
        entry
    }).collect()))
}

impl Process {
    fn spawn(
        program: PathBuf,
        guard: PathBuf,
        directory: PathBuf,
        runtime: PathBuf,
        payload: OpenRequest,
        spawner: &mpsc::Sender<SpawnRequest>,
        no_sandbox: bool,
    ) -> Result<Self> {
        private_directory(&directory)?;
        if directory
            .symlink_metadata()
            .map_err(failure)?
            .file_type()
            .is_symlink()
        {
            return Err(Error::InvalidRequest(
                "Browser profile directory must not be a symlink.",
            ));
        }
        let directory = setup::canonical(&directory)?;
        // An interrupted clear may have left a detached old cache root.
        if let Err(problem) = remove_detached(&directory) {
            eprintln!("Detached browser data was not removed: {problem}");
        }
        let profile = payload.profile.clone();
        #[cfg(target_os = "linux")]
        if !setup::socket_path_fits(&runtime) {
            return Err(Error::InvalidRequest(
                "Browser runtime socket path is too long.",
            ));
        }
        // An existing directory with this name is replaced only when it is
        // stale (its lock is free); one that another App holds is refused.
        if !setup::claim_runtime(&runtime)? {
            return Err(Error::InvalidRequest(
                "Browser runtime directory is in use by another App instance.",
            ));
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&runtime)
                .map_err(failure)?;
        }
        #[cfg(windows)]
        std::fs::create_dir(&runtime).map_err(failure)?;
        // Removed again if no guard starts; a started guard's Process owns it.
        let mut unowned = Unowned(Some(runtime.clone()));
        // Held for this browser's whole lifetime: the sweep of any App sharing
        // the root leaves a locked directory alone.
        let runtime_lock = setup::lock_runtime(&runtime)?;
        let boot = Request::Open { id: 1, payload };
        boot.validate(&profile)?;
        let bytes = serde_json::to_vec(&boot).map_err(failure)?;
        if bytes.len() > MAX_PACKET_BYTES {
            return Err(Error::InvalidRequest("Browser request is too large."));
        }
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(target_os = "linux")]
        options.mode(0o600);
        let mut file = options.open(directory.join("boot.json")).map_err(failure)?;
        file.write_all(&bytes).map_err(failure)?;
        file.flush().map_err(failure)?;
        let guard_receipt = runtime.join("guard.json");
        let mut command = guard_command(&guard);
        command.arg(&guard_receipt).arg(&program);
        // The guard owns this process group and its exact descendants. Do not
        // put it in the App's or another runner's process group.
        #[cfg(target_os = "linux")]
        command.process_group(0);
        // Tie the guard to the App from the moment it forks: if the App died
        // before this point, the guard would otherwise arm its parent-death
        // signal against its new parent and outlive the App. (Windows: the
        // spawner starts the guard inside the App's job before it runs.)
        #[cfg(target_os = "linux")]
        let app = std::process::id();
        // SAFETY: only async-signal-safe calls between fork and exec.
        #[cfg(target_os = "linux")]
        unsafe {
            command.pre_exec(move || {
                // prctl is variadic and reads unsigned longs.
                let (term, zero) = (libc::SIGTERM as libc::c_ulong, 0 as libc::c_ulong);
                if libc::prctl(libc::PR_SET_PDEATHSIG, term, zero, zero, zero) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() as u32 != app {
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            });
        }
        command.env_clear();
        // Optional observation seam for the disposable native fixture only.
        // It is never a page IPC command or enabled in a normal App build.
        if setup::inspection(&setup::environment) {
            command.env("TAKOSUMI_BROWSER_PROCESS_INSPECT", "1");
        }
        #[cfg(target_os = "linux")]
        for key in [
            "PATH",
            "USER",
            "LOGNAME",
            "LANG",
            "LC_ALL",
            "LC_CTYPE",
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "XAUTHORITY",
            "DBUS_SESSION_BUS_ADDRESS",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        #[cfg(target_os = "linux")]
        for (key, name) in [
            ("HOME", "home"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_RUNTIME_DIR", "runtime"),
        ] {
            let path = directory.join(name);
            private_directory(&path)?;
            command.env(key, path);
        }
        #[cfg(target_os = "linux")]
        {
            private_directory(&runtime.join("tmp"))?;
            command.env("TMPDIR", runtime.join("tmp"));
        }
        #[cfg(windows)]
        let stop = {
            for key in WINDOWS_ENVIRONMENT {
                if let Some(value) = std::env::var_os(key) {
                    command.env(key, value);
                }
            }
            private_directory(&runtime.join("tmp"))?;
            command
                .env("TEMP", runtime.join("tmp"))
                .env("TMP", runtime.join("tmp"));
            let name = windows::stop_event_name(&runtime);
            let stop = crate::windows_job::Event::create(&name).map_err(failure)?;
            command.env(crate::windows_job::STOP_EVENT_ENV, &name);
            stop
        };
        private_directory(&directory.join("Downloads"))?;
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(target_os = "linux")]
        options.mode(0o600);
        let output = options
            .open(directory.join("browser.log"))
            .map_err(failure)?;
        command.env("TAKOSUMI_BROWSER_PROCESS_DIRECTORY", &directory);
        #[cfg(target_os = "linux")]
        command.env("LD_LIBRARY_PATH", program.parent().unwrap());
        command
            // The browser derives CEF's own sandbox setting from this switch.
            .args(no_sandbox.then_some("--no-sandbox"))
            .args([
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-networking",
                "--disable-component-update",
                "--disable-sync",
            ]);
        #[cfg(target_os = "linux")]
        command.arg("--password-store=basic");
        command
            .arg("--lang=en-US")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(output);
        let mut child = spawn_from_parent(spawner, command)?;
        let input = child.stdin.take().expect("Owned child stdin");
        let stdout = child.stdout.take().expect("Owned child stdout");
        let (send, replies) = mpsc::channel();
        let process = Self {
            child,
            #[cfg(target_os = "linux")]
            input: Some(input),
            // Set just below, once this Process owns the started guard.
            #[cfg(windows)]
            input: None,
            replies,
            profile,
            directory,
            next: 1,
            uncertain: false,
            opening: None,
            opened: serde_json::Value::Null,
            guard_receipt,
            runtime: unowned.0.take().expect("Owned runtime directory"),
            _runtime_lock: runtime_lock,
            program,
            order: NEXT_ORDER.fetch_add(1, Ordering::SeqCst),
            #[cfg(windows)]
            stop,
            #[cfg(windows)]
            foreground_grant: None,
        };
        #[cfg(target_os = "linux")]
        nonblocking(process.input.as_ref().unwrap())?;
        // A failure here drops the Process, which asks the guard to stop.
        #[cfg(windows)]
        let process = {
            let mut process = process;
            process.input = Some(windows::Input::new(input).map_err(failure)?);
            process
        };
        let owner = process.profile.clone();
        std::thread::Builder::new()
            .name("browser-process-replies".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    let reply = match read_packet(&mut reader) {
                        Ok(Some(bytes)) => serde_json::from_slice::<Reply>(&bytes)
                            .map_err(|_| "Invalid browser reply".to_string()),
                        Ok(None) => break,
                        Err(error) => Err(error.to_string()),
                    };
                    // The person chose to open a link outside the App. The browser
                    // runs with its profile's HOME and XDG directories, so the App
                    // opens it with the person's own desktop; never a command reply.
                    // Another profile's request is a protocol fault, as for closes.
                    let reply = match reply {
                        Ok(Reply::OpenExternal { profile, .. }) if profile != owner => {
                            Err("Foreign external link request".to_string())
                        }
                        Ok(Reply::OpenExternal { url, .. }) => {
                            if external_link(&url) {
                                open_externally(&url);
                            } else {
                                eprintln!("An invalid external link request was ignored.");
                            }
                            continue;
                        }
                        other => other,
                    };
                    let failed = reply.is_err();
                    if send.send(reply).is_err() || failed {
                        break;
                    }
                }
            })
            .map_err(failure)?;
        Ok(process)
    }
    fn send(&mut self, request: Request, deadline: std::time::Instant) -> Result<()> {
        self.write_request(request, deadline)
            .map_err(|unsent| unsent.error)
    }
    fn write_request(
        &mut self,
        request: Request,
        deadline: std::time::Instant,
    ) -> std::result::Result<(), Unsent> {
        let refused = |error| Unsent {
            error,
            closed: false,
        };
        request.validate(&self.profile).map_err(refused)?;
        let mut bytes = serde_json::to_vec(&request)
            .map_err(failure)
            .map_err(refused)?;
        if bytes.len() > MAX_PACKET_BYTES {
            return Err(refused(Error::InvalidRequest(
                "Browser request is too large.",
            )));
        }
        bytes.push(b'\n');
        let Some(input) = self.input.as_mut() else {
            return Err(Unsent {
                error: Error::InvalidRequest("Browser pipe is closed."),
                closed: true,
            });
        };
        write_input(input, &bytes, deadline).map_err(|error| Unsent {
            closed: matches!(
                error.kind(),
                std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::WriteZero
            ),
            error: failure(error),
        })
    }
    fn receive(
        &mut self,
        id: u64,
        deadline: std::time::Instant,
    ) -> std::result::Result<Reply, Wait> {
        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .ok_or(Wait::TimedOut)?;
            let reply = match self.replies.recv_timeout(remaining) {
                Ok(Ok(reply)) => reply,
                Ok(Err(message)) => return Err(Wait::Fault(failure(message))),
                Err(mpsc::RecvTimeoutError::Timeout) => return Err(Wait::TimedOut),
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(Wait::Closed),
            };
            if let Reply::Closed { id: 0, profile, .. } = &reply {
                if profile != &self.profile {
                    return Err(Wait::Fault(Error::InvalidRequest(
                        "Foreign browser close reply.",
                    )));
                }
                continue;
            }
            let actual = match &reply {
                // The reply reader consumes these; one here is a protocol fault.
                Reply::OpenExternal { .. } => {
                    return Err(Wait::Fault(Error::InvalidRequest(
                        "Unexpected external link request.",
                    )))
                }
                Reply::Opened { id, .. }
                | Reply::Closed { id, .. }
                | Reply::Stopped { id }
                | Reply::Error { id, .. }
                | Reply::CloseBlocked { id, .. }
                | Reply::Cancelled { id, .. } => *id,
            };
            // A reply to an earlier request that timed out (e.g. a person answering
            // a beforeunload dialog late) is stale, not a protocol fault.
            if actual != 0 && actual < id && !matches!(reply, Reply::Error { .. }) {
                continue;
            }
            if actual != id {
                return Err(Wait::Fault(Error::InvalidRequest(
                    "Browser response identity mismatch.",
                )));
            }
            if let Reply::Error { message, .. } = reply {
                return Err(Wait::Rejected(message));
            }
            return Ok(reply);
        }
    }
    fn expect_open(&mut self, id: u64, key: &str) -> Result<()> {
        self.expect_open_until(id, key, std::time::Instant::now() + Duration::from_secs(20))
    }
    fn expect_open_until(
        &mut self,
        id: u64,
        key: &str,
        deadline: std::time::Instant,
    ) -> Result<()> {
        self.opening = Some(key.to_owned());
        let result = match self.receive(id, deadline) {
            Ok(reply) => self.confirm_opened(reply, key),
            // The browser refused this creation outright: nothing is pending.
            Err(Wait::Rejected(message)) => {
                self.opening = None;
                return Err(failure(message));
            }
            Err(wait) => Err(wait.into()),
        };
        self.uncertain = result.is_err();
        result
    }
    /// Record a creation reply for this browser's own native child, profile
    /// cache and key; anything else is unconfirmed.
    fn confirm_opened(&mut self, reply: Reply, key: &str) -> Result<()> {
        match reply {
            Reply::Opened {
                key: actual,
                profile,
                browser_id,
                cache_path,
                window_id,
                native_pid,
                ..
            } if actual == key
                && profile == self.profile
                && browser_id > 0
                && window_id > 0
                && native_pid > 0
                && native_owned(native_pid, &self.child, &self.program)
                && same_path(
                    &cache_path,
                    &self.directory.join("cef-profiles").join(&self.profile),
                ) =>
            {
                self.opened = serde_json::json!({"key":actual,"profile":profile,"browserId":browser_id,"nativeCache":cache_path,"windowId":window_id,"nativePid":native_pid});
                self.opening = None;
                Ok(())
            }
            _ => Err(Error::InvalidRequest(
                "Browser profile creation was not confirmed.",
            )),
        }
    }
    fn open(&mut self, payload: OpenRequest) -> Result<()> {
        if self.uncertain {
            return Err(Error::InvalidRequest(
                "Previous browser command is incomplete; profile data is retained.",
            ));
        }
        let key = payload.key.clone();
        self.next = self
            .next
            .checked_add(1)
            .ok_or(Error::InvalidRequest("Browser command limit reached."))?;
        let id = self.next;
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        // Windows: the browser raises its reused window before it replies;
        // pass it the foreground right the App holds while the home is used.
        #[cfg(windows)]
        if let Some(pid) = self.opened["nativePid"]
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
        {
            self.foreground_grant = Some(windows::allow_foreground(pid));
        }
        if let Err(error) = self.send(Request::Open { id, payload }, deadline) {
            self.uncertain = true;
            return Err(error);
        }
        self.expect_open_until(id, &key, deadline)
    }
    fn shutdown(&mut self) -> Result<bool> {
        // A person answers each window's beforeunload in turn: the reply bound
        // is interactive, unlike the write bound.
        self.shutdown_until(std::time::Instant::now() + Duration::from_secs(300))
    }
    /// Any failure leaves the browser in doubt, except an error it answered.
    fn shutdown_until(&mut self, reply_deadline: std::time::Instant) -> Result<bool> {
        self.shutdown_within(WRITE_BOUND, reply_deadline)
    }
    fn shutdown_within(
        &mut self,
        write_bound: Duration,
        reply_deadline: std::time::Instant,
    ) -> Result<bool> {
        let mut rejected = false;
        let result = self.shutdown_attempt(write_bound, reply_deadline, &mut rejected);
        if result.is_err() && !rejected {
            self.uncertain = true;
        }
        result
    }
    fn shutdown_attempt(
        &mut self,
        write_bound: Duration,
        reply_deadline: std::time::Instant,
        rejected: &mut bool,
    ) -> Result<bool> {
        if let Some(status) = self.child.try_wait().map_err(failure)? {
            return if status.success() {
                self.normal_guard().map(|()| true)
            } else {
                Err(Error::InvalidRequest("Browser process ended abnormally."))
            };
        }
        self.next = self
            .next
            .checked_add(1)
            .ok_or(Error::InvalidRequest("Browser command limit reached."))?;
        let id = self.next;
        self.opening = None;
        let now = std::time::Instant::now();
        // A browser whose last window was just closed is already ending: its
        // pipe or reply channel closes. Only then accept a normal end; a
        // protocol fault stays an error.
        match self.write_request(Request::Shutdown { id }, now + write_bound) {
            Ok(()) => {}
            Err(Unsent {
                error,
                closed: true,
            }) => {
                return self
                    .ended_within(STOP_FINISH)
                    .map(|()| true)
                    .map_err(|_| error);
            }
            // It never agreed to stop (it did not even read the request within
            // the write bound): no reason to wait for its end.
            Err(Unsent { error, .. }) => return Err(error),
        }
        // A timeout leaves any late answer queued for `recover`.
        let reply = match self.receive(id, reply_deadline) {
            Ok(reply) => reply,
            Err(Wait::Closed) => {
                return self
                    .ended_within(STOP_FINISH)
                    .map(|()| true)
                    .map_err(|_| Wait::Closed.into());
            }
            Err(Wait::Rejected(message)) => {
                *rejected = true;
                return Err(failure(message));
            }
            Err(wait) => return Err(wait.into()),
        };
        match reply {
            Reply::Cancelled { profile, .. } if profile == self.profile => return Ok(false),
            Reply::CloseBlocked { profile, .. } if profile == self.profile => {
                eprintln!("A browser window is still opening; the quit stopped.");
                return Ok(false);
            }
            Reply::Stopped { .. } => {}
            _ => {
                return Err(Error::InvalidRequest("Browser shutdown was not confirmed."));
            }
        }
        self.input.take();
        // The person already agreed to leave and every window has closed.
        let deadline = std::time::Instant::now() + STOP_FINISH;
        while std::time::Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().map_err(failure)? {
                return if status.success() {
                    self.normal_guard().map(|()| true)
                } else {
                    Err(Error::InvalidRequest("Browser process exited abnormally."))
                };
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Err(Error::InvalidRequest(
            "Browser process did not finish shutdown.",
        ))
    }
    /// Settle a browser left in doubt by an earlier timeout before using it:
    /// a late reply to its last request, or its own normal end. Returns true
    /// when it has ended (the caller drops it); a late cancel, refusal, error
    /// or verified creation makes it usable. Other replies keep it in doubt,
    /// and a fault is an error.
    fn recover(&mut self) -> Result<bool> {
        if !self.uncertain {
            return Ok(false);
        }
        if let Some(status) = self.child.try_wait().map_err(failure)? {
            return if status.success() {
                self.normal_guard().map(|()| true)
            } else {
                Err(Error::InvalidRequest("Browser process ended abnormally."))
            };
        }
        loop {
            let reply = match self.replies.try_recv() {
                Ok(Ok(reply)) => reply,
                Ok(Err(message)) => {
                    self.uncertain = true;
                    return Err(failure(message));
                }
                Err(_) => break,
            };
            match reply {
                Reply::Cancelled { id, ref profile } | Reply::CloseBlocked { id, ref profile }
                    if id == self.next && *profile == self.profile =>
                {
                    self.uncertain = false;
                }
                Reply::Stopped { id } if id == self.next => {
                    self.input.take();
                    return self.ended_within(STOP_FINISH).map(|()| true);
                }
                Reply::Opened { id, .. } if id == self.next => {
                    if let Some(key) = self.opening.clone() {
                        if self.confirm_opened(reply, &key).is_ok() {
                            self.uncertain = false;
                        }
                    }
                }
                // The browser rejected the last request outright: every native
                // error path leaves no pending stop or creation behind.
                Reply::Error { id, .. } if id == self.next => {
                    self.uncertain = false;
                    self.opening = None;
                }
                _ => {}
            }
        }
        Ok(false)
    }
    /// The browser ended normally within the bound, with a complete receipt.
    fn ended_within(&mut self, bound: Duration) -> Result<()> {
        let deadline = std::time::Instant::now() + bound;
        loop {
            if let Some(status) = self.child.try_wait().map_err(failure)? {
                return if status.success() {
                    self.normal_guard()
                } else {
                    Err(Error::InvalidRequest("Browser process ended abnormally."))
                };
            }
            if std::time::Instant::now() >= deadline {
                return Err(Error::InvalidRequest(
                    "Browser process did not finish shutdown.",
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn normal_guard(&self) -> Result<()> {
        let proof: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&self.guard_receipt).map_err(failure)?)
                .map_err(failure)?;
        require_normal_guard(&proof)
    }
}
fn require_normal_guard(proof: &serde_json::Value) -> Result<()> {
    if proof["rootExit"] == 0
        && proof["complete"] == true
        && proof["kernelNoChildren"] == true
        && proof["remaining"]
            .as_array()
            .is_some_and(|items| items.is_empty())
        && proof["forcedCleanup"] == 0
        && proof.get("error") == Some(&serde_json::Value::Null)
        && proof["cleanupErrors"]
            .as_array()
            .is_some_and(|items| items.is_empty())
    {
        Ok(())
    } else {
        Err(Error::InvalidRequest(
            "Browser descendants did not close normally; profile retained.",
        ))
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    fn an_unresolvable_browser_leaves_the_app_running_with_its_reason() {
        // The test executable has no packaged `cef/` resources beside it, so
        // locating the browser fails; installing must not.
        let app = tauri::test::mock_app();
        install(app.handle()).unwrap();
        assert!(app.try_state::<Backend>().is_none());
        let reason = unavailable(app.handle()).expect("an unavailable reason");
        assert!(
            reason.starts_with("The desktop browser is unavailable: "),
            "{reason}"
        );
        assert!(
            reason.contains("Browser process executable is unavailable"),
            "{reason}"
        );
        // The home then offers no data deletion, which would be refused.
        assert!(!super::super::capabilities_of(app.handle()).clear_profile);
        let ready = tauri::test::mock_app();
        assert!(super::super::capabilities_of(ready.handle()).clear_profile);
    }
    #[test]
    fn a_zero_exit_never_substitutes_for_complete_normal_descendant_proof() {
        let valid = serde_json::json!({"rootExit":0,"complete":true,"kernelNoChildren":true,"remaining":[],"forcedCleanup":0,"error":null,"cleanupErrors":[]});
        require_normal_guard(&valid).unwrap();
        for (field, value) in [
            ("rootExit", serde_json::json!(1)),
            ("complete", serde_json::json!(false)),
            ("kernelNoChildren", serde_json::json!(false)),
            ("remaining", serde_json::json!([123])),
            ("forcedCleanup", serde_json::json!(1)),
            ("error", serde_json::json!("failed")),
            ("cleanupErrors", serde_json::json!(["failed"])),
        ] {
            let mut bad = valid.clone();
            bad[field] = value;
            assert!(require_normal_guard(&bad).is_err(), "{field}");
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(require_normal_guard(&missing).is_err(), "missing {field}");
        }
    }
    /// Asks for SIGTERM on parent death, as the browser guard does, with the
    /// default disposition even if the runner ignores SIGTERM. Reports once
    /// armed: argv[1] "ready" prints to stdout; any other path is a marker file
    /// beside it (the guard receipt position in `Process::spawn`).
    const PARENT_DEATH_CHILD: &str = "import ctypes,pathlib,signal,sys,time\nsignal.signal(signal.SIGTERM,signal.SIG_DFL)\nassert ctypes.CDLL(None).prctl(1,signal.SIGTERM,0,0,0)==0\nif sys.argv[1]=='ready': print('ready',flush=True)\nelse: pathlib.Path(sys.argv[1]).with_name('armed').write_text('1')\ntime.sleep(30)";
    fn parent_death_child() -> Command {
        let mut command = Command::new("python3");
        command
            .args(["-B", "-c", PARENT_DEATH_CHILD, "ready"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        command
    }
    fn ready(mut child: Child) -> Child {
        let mut line = String::new();
        std::io::BufRead::read_line(&mut BufReader::new(child.stdout.take().unwrap()), &mut line)
            .unwrap();
        assert_eq!(line, "ready\n");
        child
    }
    fn exit_within(child: &mut Child, limit: Duration) -> Option<std::process::ExitStatus> {
        let deadline = std::time::Instant::now() + limit;
        while std::time::Instant::now() < deadline {
            if let Some(status) = child.try_wait().unwrap() {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }
    #[test]
    fn a_retired_forking_thread_terminates_a_parent_death_guarded_child() {
        // Control for the hazard: the kernel ties the link to the forking thread.
        let mut child = std::thread::spawn(|| ready(parent_death_child().spawn().unwrap()))
            .join()
            .unwrap();
        let status = exit_within(&mut child, Duration::from_secs(5));
        if status.is_none() {
            child.kill().unwrap();
            child.wait().unwrap();
        }
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            status.and_then(|status| status.signal()),
            Some(libc::SIGTERM)
        );
    }
    #[test]
    fn guards_forked_by_the_dedicated_parent_outlive_the_requesting_thread() {
        let parent = spawner().unwrap();
        let requester = parent.clone();
        let mut child = std::thread::spawn(move || {
            ready(spawn_from_parent(&requester, parent_death_child()).unwrap())
        })
        .join()
        .unwrap();
        // The signal follows the forking thread's exit at once; 2 s is ample.
        let status = exit_within(&mut child, Duration::from_secs(2));
        if status.is_none() {
            child.kill().unwrap();
            child.wait().unwrap();
        }
        assert_eq!(status, None, "a retired requester must not end the guard");
        drop(parent);
    }
    #[test]
    fn open_requests_fork_the_actual_guard_from_the_dedicated_parent() {
        // Drive the real call site with a stand-in guard that only arms its
        // parent-death link. Forking on the requester would end it below.
        let root = std::env::temp_dir().join(format!("sumi-pa-{}", std::process::id()));
        private_directory(&root).unwrap();
        let guard = root.join("guard.py");
        std::fs::write(&guard, PARENT_DEATH_CHILD).unwrap();
        let runtime = root.join("r");
        let payload = serde_json::from_value(serde_json::json!({
            "key":"web-home:11111111-1111-4111-8111-111111111111:https://excalidraw.com",
            "url":"https://excalidraw.com/","title":"Work","profile":"11111111-1111-4111-8111-111111111111","webHome":true
        }))
        .unwrap();
        let parent = spawner().unwrap();
        let (requester, armed) = (parent.clone(), runtime.join("armed"));
        let mut process = std::thread::spawn(move || {
            let process = Process::spawn(
                PathBuf::from("/bin/true"),
                guard,
                root.join("profile"),
                runtime,
                payload,
                &requester,
                true,
            )
            .unwrap();
            // Keep the requester alive until the link is armed, so a fork on
            // this thread would be signalled when it returns.
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while !armed.exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(armed.exists(), "stand-in guard never armed");
            process
        })
        .join()
        .unwrap();
        let status = exit_within(&mut process.child, Duration::from_secs(2));
        drop(process); // Drop's SIGTERM path ends the default-disposition stand-in.
        std::fs::remove_dir_all(
            std::env::temp_dir().join(format!("sumi-pa-{}", std::process::id())),
        )
        .unwrap();
        assert_eq!(
            status, None,
            "the actual open path must not tie guards to a retiring thread"
        );
        drop(parent);
    }
    #[test]
    fn only_a_py_guard_runs_under_python() {
        let fixture = guard_command(std::path::Path::new("/opt/fixture/guard.py"));
        assert_eq!(fixture.get_program(), "python3");
        assert_eq!(
            fixture.get_args().collect::<Vec<_>>(),
            ["-B", "/opt/fixture/guard.py"]
        );
        let product = guard_command(std::path::Path::new("/opt/app/takosumi-browser-guard"));
        assert_eq!(product.get_program(), "/opt/app/takosumi-browser-guard");
        assert_eq!(product.get_args().count(), 0);
    }
    /// A stand-in product guard that is not Python: run under python3 it would
    /// fail to parse and never record how it was started. It records its argv
    /// beside the receipt position, then execs a reader of its parent-death
    /// signal (kept across exec; neither sh nor python sets one), which waits
    /// for the control pipe to close.
    const DIRECT_GUARD: &str = r#"#!/bin/sh
printf '%s\n' "$0" "$@" > "${1%/*}/argv"
exec python3 -I -B -c 'import ctypes,os,sys
d=os.path.dirname(sys.argv[1])
s=ctypes.c_int(-1)
ctypes.CDLL(None).prctl(2,ctypes.byref(s),0,0,0)
open(d+"/pdeathsig","w").write(str(s.value))
open(d+"/ready","w").close()
sys.stdin.read()' "$1"
"#;
    /// Start the stand-in product guard through the actual open path and
    /// return the argv it recorded.
    fn direct_guard_argv(no_sandbox: bool) -> Vec<String> {
        let root = std::env::temp_dir().join(format!(
            "sumi-pa-direct-{no_sandbox}-{}",
            std::process::id()
        ));
        private_directory(&root).unwrap();
        let guard = root.join("takosumi-browser-guard");
        // Written and made executable by a process that has exited, so no
        // sibling test's fork can still hold it open for writing (ETXTBSY).
        let written = Command::new("python3")
            .args([
                "-I",
                "-B",
                "-c",
                "import os,sys\nopen(sys.argv[1],'w').write(sys.argv[2])\nos.chmod(sys.argv[1],0o700)",
            ])
            .arg(&guard)
            .arg(DIRECT_GUARD)
            .status()
            .unwrap();
        assert!(written.success());
        let runtime = root.join("r");
        let payload = serde_json::from_value(serde_json::json!({
            "key":"web-home:11111111-1111-4111-8111-111111111111:https://excalidraw.com",
            "url":"https://excalidraw.com/","title":"Work","profile":"11111111-1111-4111-8111-111111111111","webHome":true
        }))
        .unwrap();
        let parent = spawner().unwrap();
        let process = Process::spawn(
            PathBuf::from("/bin/true"),
            guard.clone(),
            root.join("profile"),
            runtime.clone(),
            payload,
            &parent,
            no_sandbox,
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !runtime.join("ready").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let recorded = std::fs::read_to_string(runtime.join("argv"));
        let pdeathsig = std::fs::read_to_string(runtime.join("pdeathsig"));
        drop(process); // Closing the control pipe ends the stand-in.
        std::fs::remove_dir_all(&root).unwrap();
        let recorded = recorded.expect("the guard binary was not executed directly");
        // Armed by the adapter between fork and exec, before the guard runs.
        assert_eq!(pdeathsig.unwrap(), libc::SIGTERM.to_string());
        let lines = recorded.lines().map(str::to_owned).collect::<Vec<_>>();
        let receipt = runtime.join("guard.json");
        assert_eq!(
            lines[..3],
            [
                guard.to_str().unwrap(),
                receipt.to_str().unwrap(),
                "/bin/true"
            ]
        );
        drop(parent);
        lines
    }
    #[test]
    fn open_requests_execute_a_guard_binary_directly() {
        let lines = direct_guard_argv(true);
        assert!(lines.iter().any(|line| line == "--no-sandbox"), "{lines:?}");
        assert!(
            lines.iter().any(|line| line == "--no-first-run"),
            "{lines:?}"
        );
    }
    #[test]
    fn a_sandboxed_browser_is_started_without_the_no_sandbox_switch() {
        let lines = direct_guard_argv(false);
        assert!(
            !lines.iter().any(|line| line == "--no-sandbox"),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|line| line == "--no-first-run"),
            "{lines:?}"
        );
    }
    /// A stand-in guard that writes a normal receipt and ends at once.
    fn ending_guard(root: &std::path::Path) -> PathBuf {
        let guard = root.join("guard.py");
        std::fs::write(
            &guard,
            format!("import json,sys\nopen(sys.argv[1],'w').write(json.dumps({NORMAL_RECEIPT}))\n"),
        )
        .unwrap();
        guard
    }
    fn work_payload() -> OpenRequest {
        serde_json::from_value(serde_json::json!({
            "key":"web-home:11111111-1111-4111-8111-111111111111:https://excalidraw.com",
            "url":"https://excalidraw.com/","title":"Work","profile":"11111111-1111-4111-8111-111111111111","webHome":true
        }))
        .unwrap()
    }
    #[test]
    fn an_ended_browsers_runtime_directory_goes_with_it_and_a_stale_one_is_replaced() {
        let root = std::env::temp_dir().join(format!("sumi-rt-{}", std::process::id()));
        private_directory(&root.join("outside")).unwrap();
        let guard = ending_guard(&root);
        let runtime = root.join("r");
        // Left by an ended App instance with the same pid (its lock is free):
        // junk, a socket directory and a link to outside, never followed.
        private_directory(&runtime.join("tmp").join("org.chromium.Chromium.x")).unwrap();
        std::fs::write(runtime.join("guard.json"), b"stale").unwrap();
        std::os::unix::fs::symlink(root.join("outside"), runtime.join("tmp").join("out")).unwrap();
        drop(setup::lock_runtime(&runtime).unwrap());
        // One that another App instance holds is refused, never replaced.
        let held_dir = root.join("held");
        private_directory(&held_dir).unwrap();
        let held = setup::lock_runtime(&held_dir).unwrap();
        let parent = spawner().unwrap();
        let refused = Process::spawn(
            PathBuf::from("/bin/true"),
            root.join("guard.py"),
            root.join("profile"),
            held_dir.clone(),
            work_payload(),
            &parent,
            true,
        );
        let held_intact = held_dir.join("lock").exists();
        drop(held);
        let mut process = Process::spawn(
            PathBuf::from("/bin/true"),
            guard,
            root.join("profile"),
            runtime.clone(),
            work_payload(),
            &parent,
            true,
        )
        .expect("a stale runtime directory never makes the start fail");
        // The start recreates `tmp/`; nothing stale remains in it.
        let stale_gone = !runtime.join("tmp/org.chromium.Chromium.x").exists()
            && runtime.join("tmp/out").symlink_metadata().is_err();
        let status = exit_within(&mut process.child, Duration::from_secs(10));
        let verified = process.normal_guard();
        drop(process);
        let kept = runtime.exists();
        let outside = root.join("outside").is_dir();
        std::fs::remove_dir_all(&root).unwrap();
        assert!(
            refused.is_err() && held_intact,
            "a held runtime is never replaced"
        );
        assert!(stale_gone, "the stale directory was removed first");
        assert!(status.is_some_and(|status| status.success()));
        verified.unwrap();
        assert_eq!(
            kept,
            setup::KEEP_ENDED_RUNTIME,
            "removed with its browser, kept only for the fixture runner"
        );
        assert!(
            outside,
            "links inside a runtime directory are never followed"
        );
        drop(parent);
    }
    #[test]
    fn a_browser_that_never_starts_leaves_no_runtime_directory() {
        let root = std::env::temp_dir().join(format!("sumi-rt-none-{}", std::process::id()));
        private_directory(&root).unwrap();
        let runtime = root.join("r");
        let parent = spawner().unwrap();
        let started = Process::spawn(
            PathBuf::from("/bin/true"),
            root.join("no-such-guard"),
            root.join("profile"),
            runtime.clone(),
            work_payload(),
            &parent,
            true,
        );
        let left = runtime.exists();
        std::fs::remove_dir_all(&root).unwrap();
        assert!(started.is_err());
        assert!(
            !left,
            "a guard that never started owns no runtime directory"
        );
        drop(parent);
    }
    #[test]
    fn clearing_site_data_removes_only_that_profiles_cache_and_never_follows_links() {
        let root = std::env::temp_dir().join(format!("sumi-clear-{}", std::process::id()));
        let (work, personal) = (
            "11111111-1111-4111-8111-111111111111",
            "22222222-2222-4222-8222-222222222222",
        );
        let outside = root.join("outside");
        for path in [
            root.join(work).join("cef-profiles").join(work),
            root.join(work).join("Downloads"),
            root.join(personal).join("cef-profiles").join(personal),
            outside.clone(),
        ] {
            private_directory(&path).unwrap();
        }
        std::fs::write(
            root.join(work)
                .join("cef-profiles")
                .join(work)
                .join("Cookies"),
            b"x",
        )
        .unwrap();
        std::fs::write(root.join(work).join("Downloads").join("keep.txt"), b"keep").unwrap();
        std::fs::write(
            root.join(personal)
                .join("cef-profiles")
                .join(personal)
                .join("Cookies"),
            b"y",
        )
        .unwrap();
        std::fs::write(outside.join("target.txt"), b"outside").unwrap();
        std::os::unix::fs::symlink(&outside, root.join(work).join("cef-profiles").join("link"))
            .unwrap();
        // A leftover detached directory from an interrupted earlier clear.
        private_directory(&root.join(work).join(format!("{CLEARING}0-0"))).unwrap();

        clear_site_data(&root, work).unwrap();
        assert!(!root.join(work).join("cef-profiles").exists());
        assert!(std::fs::read_dir(root.join(work))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(CLEARING)));
        assert_eq!(
            std::fs::read(root.join(work).join("Downloads").join("keep.txt")).unwrap(),
            b"keep"
        );
        assert_eq!(
            std::fs::read(
                root.join(personal)
                    .join("cef-profiles")
                    .join(personal)
                    .join("Cookies")
            )
            .unwrap(),
            b"y"
        );
        assert_eq!(
            std::fs::read(outside.join("target.txt")).unwrap(),
            b"outside"
        );

        // A linked cache root or profile directory is refused, not followed.
        std::fs::remove_dir_all(root.join(personal).join("cef-profiles")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join(personal).join("cef-profiles")).unwrap();
        assert!(clear_site_data(&root, personal).is_err());
        std::os::unix::fs::symlink(&outside, root.join("33333333-3333-4333-8333-333333333333"))
            .unwrap();
        assert!(clear_site_data(&root, "33333333-3333-4333-8333-333333333333").is_err());
        assert_eq!(
            std::fs::read(outside.join("target.txt")).unwrap(),
            b"outside"
        );
        // CEF's singleton of a live browser (this test process) refuses the clear;
        // a stale one left by an ended process does not.
        let live = root.join("55555555-5555-4555-8555-555555555555");
        private_directory(&live.join("cef-profiles")).unwrap();
        std::fs::write(live.join("cef-profiles").join("Cookies"), b"z").unwrap();
        let host = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap();
        let host = host.trim();
        let lock = live.join("cef-profiles").join("SingletonLock");
        let mut ended = Command::new("true").spawn().unwrap();
        let ended_pid = ended.id();
        ended.wait().unwrap();
        for target in [
            format!("{host}-{}", std::process::id()), // live browser on this host
            format!("other-host-{ended_pid}"),        // another host: cannot tell
            "not-a-pid".to_owned(),                   // unparsable
        ] {
            let _ = std::fs::remove_file(&lock);
            std::os::unix::fs::symlink(&target, &lock).unwrap();
            match clear_site_data(&root, "55555555-5555-4555-8555-555555555555") {
                Err(Error::InvalidRequest(message)) => assert_eq!(message, STILL_OPEN, "{target}"),
                other => panic!("{target}: {other:?}"),
            }
            assert_eq!(
                std::fs::read(live.join("cef-profiles").join("Cookies")).unwrap(),
                b"z"
            );
        }
        std::fs::remove_file(&lock).unwrap();
        std::os::unix::fs::symlink(format!("{host}-{ended_pid}"), &lock).unwrap();
        clear_site_data(&root, "55555555-5555-4555-8555-555555555555").unwrap();
        assert!(!live.join("cef-profiles").exists());
        // Missing data is already clear.
        assert!(clear_site_data(&root, "44444444-4444-4444-8444-444444444444").is_ok());
        std::fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn a_just_closed_profile_is_not_counted_as_still_open() {
        // Two real Process::spawn children with stand-in guards: Work lingers;
        // Personal's last window closed, so it writes a normal receipt and ends.
        let root = std::env::temp_dir().join(format!("sumi-settle-{}", std::process::id()));
        private_directory(&root).unwrap();
        let lingering = root.join("lingering.py");
        std::fs::write(&lingering, "import time\ntime.sleep(30)\n").unwrap();
        let exiting = root.join("exiting.py");
        std::fs::write(&exiting, "import json,pathlib,sys,time\npathlib.Path(sys.argv[1]).with_name('started').write_text('1')\ntime.sleep(0.3)\nopen(sys.argv[1],'w').write(json.dumps({'rootExit':0,'complete':True,'kernelNoChildren':True,'remaining':[],'forcedCleanup':0,'error':None,'cleanupErrors':[]}))\n").unwrap();
        let (work, personal) = (
            "11111111-1111-4111-8111-111111111111",
            "22222222-2222-4222-8222-222222222222",
        );
        let parent = spawner().unwrap();
        let mut processes = BTreeMap::new();
        for (index, (profile, guard)) in [(work, &lingering), (personal, &exiting)]
            .into_iter()
            .enumerate()
        {
            let payload = serde_json::from_value(serde_json::json!({
                "key":format!("web-home:{profile}:https://excalidraw.com"),
                "url":"https://excalidraw.com/","title":"App","profile":profile,"webHome":true
            }))
            .unwrap();
            let process = Process::spawn(
                PathBuf::from("/bin/true"),
                guard.clone(),
                root.join(profile),
                root.join(format!("r{index}")),
                payload,
                &parent,
                true,
            )
            .unwrap();
            processes.insert(profile.to_owned(), process);
        }
        let mut state = State {
            program: PathBuf::from("/bin/true"),
            guard: lingering.clone(),
            directory: root.clone(),
            runtime_root: root.clone(),
            no_sandbox: true,
            processes,
            spawner: parent.clone(),
        };
        // Python start-up under host I/O load must not eat the bound under test.
        marker(&root.join("r1").join("started"));
        let started = std::time::Instant::now();
        settle_exiting(&mut state, started + Duration::from_secs(2)).unwrap();
        assert_eq!(
            state
                .processes
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec![work]
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "settled as soon as one remained"
        );
        // A clear of the still-open Work is refused after the bounded wait.
        match release_ended(
            &mut state,
            work,
            std::time::Instant::now() + Duration::from_millis(200),
        ) {
            Err(Error::InvalidRequest(message)) => assert_eq!(message, STILL_OPEN),
            other => panic!("{other:?}"),
        }
        assert!(state.processes.contains_key(work));
        drop(state); // Drop's SIGTERM path ends the lingering stand-in.
        std::fs::remove_dir_all(&root).unwrap();
    }
    const NORMAL_RECEIPT: &str = "{'rootExit':0,'complete':True,'kernelNoChildren':True,'remaining':[],'forcedCleanup':0,'error':None,'cleanupErrors':[]}";
    /// Poll for a stand-in's marker file; Python start-up under host I/O load
    /// must not eat a bound under test.
    fn marker(path: &std::path::Path) -> String {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(text) = std::fs::read_to_string(path) {
                if !text.is_empty() {
                    return text;
                }
            }
            assert!(std::time::Instant::now() < deadline, "missing {path:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    /// A stand-in guard: starts the "native" child (the program argument) and
    /// another child with a different executable, then ends normally once its
    /// input closes. SIGTERM (Drop) also ends both children.
    fn recovering_guard(root: &std::path::Path) -> PathBuf {
        let guard = root.join("guard.py");
        std::fs::write(&guard, format!("import json,pathlib,signal,subprocess,sys\nhere=pathlib.Path(sys.argv[1])\nnative=subprocess.Popen([sys.argv[2],'30'])\nother=subprocess.Popen([sys.executable,'-c','import time;time.sleep(30)'])\ndef end():\n    native.kill();other.kill();native.wait();other.wait()\ndef stop(*_):\n    end();sys.exit(0)\nsignal.signal(signal.SIGTERM,stop)\nhere.with_name('other').write_text(str(other.pid))\nhere.with_name('native').write_text(str(native.pid))\nsys.stdin.read()\nsignal.signal(signal.SIGTERM,signal.SIG_IGN)\nend()\nopen(sys.argv[1],'w').write(json.dumps({NORMAL_RECEIPT}))\n")).unwrap();
        guard
    }
    /// A live stand-in browser for `profile` with injected replies, as after a
    /// request with id 2 timed out.
    fn in_doubt(
        root: &std::path::Path,
        guard: &std::path::Path,
        program: &std::path::Path,
        parent: &mpsc::Sender<SpawnRequest>,
        index: usize,
        profile: &str,
    ) -> (Process, mpsc::Sender<std::result::Result<Reply, String>>) {
        let payload = serde_json::from_value(serde_json::json!({
            "key":format!("web-home:{profile}:https://excalidraw.com"),
            "url":"https://excalidraw.com/","title":"App","profile":profile,"webHome":true
        }))
        .unwrap();
        let mut process = Process::spawn(
            program.to_owned(),
            guard.to_owned(),
            root.join(format!("{index}")).join(profile),
            root.join(format!("r{index}")),
            payload,
            parent,
            true,
        )
        .unwrap();
        marker(&process.guard_receipt.with_file_name("native"));
        let (send, replies) = mpsc::channel();
        process.replies = replies;
        process.next = 2;
        process.uncertain = true;
        (process, send)
    }
    #[test]
    fn a_late_answer_recovers_a_profile_left_in_doubt() {
        let root = std::env::temp_dir().join(format!("sumi-recover-{}", std::process::id()));
        private_directory(&root).unwrap();
        let (work, personal) = (
            "11111111-1111-4111-8111-111111111111",
            "22222222-2222-4222-8222-222222222222",
        );
        let guard = recovering_guard(&root);
        let program = std::fs::canonicalize("/bin/sleep").unwrap();
        let parent = spawner().unwrap();
        let mut index = 0;
        let mut spawn = |profile: &str| {
            index += 1;
            in_doubt(&root, &guard, &program, &parent, index, profile)
        };
        let cancelled = |id, profile: &str| {
            Ok(Reply::Cancelled {
                id,
                profile: profile.to_owned(),
            })
        };
        let blocked = |id, profile: &str| {
            Ok(Reply::CloseBlocked {
                id,
                profile: profile.to_owned(),
            })
        };
        let error = |id| {
            Ok(Reply::Error {
                id,
                message: "Native browser creation rejected".into(),
            })
        };
        // A cancel: only the answer to the last request, for this profile, counts.
        let (mut process, send) = spawn(work);
        assert!(!process.recover().unwrap(), "no answer yet");
        assert!(process.uncertain, "stays in doubt without an answer");
        for stale in [
            cancelled(1, work),
            cancelled(2, personal),
            blocked(1, work),
            blocked(2, personal),
            error(1),
            Ok(Reply::Stopped { id: 1 }),
        ] {
            send.send(stale).unwrap();
        }
        assert!(!process.recover().unwrap(), "stale or foreign answers");
        assert!(
            process.uncertain,
            "stale or foreign answers keep it in doubt"
        );
        assert!(
            process.child.try_wait().unwrap().is_none(),
            "and keep the browser"
        );
        send.send(cancelled(2, work)).unwrap();
        assert!(
            !process.recover().unwrap(),
            "a late cancel keeps the browser"
        );
        assert!(!process.uncertain, "and makes it usable again");
        drop(process);
        // A refusal (a window was still opening) or an error leaves nothing pending.
        for answer in [blocked(2, work), error(2)] {
            let (mut process, send) = spawn(work);
            process.opening = Some("pending".into());
            send.send(answer).unwrap();
            assert!(
                !process.recover().unwrap(),
                "a late refusal or error keeps the browser"
            );
            assert!(!process.uncertain, "and makes it usable again");
            drop(process);
        }
        // A fault is not an answer, even after one.
        let (mut process, send) = spawn(work);
        send.send(cancelled(2, work)).unwrap();
        send.send(Err("Invalid browser reply".into())).unwrap();
        assert!(process.recover().is_err(), "a fault is an error");
        assert!(process.uncertain, "and keeps it in doubt");
        drop(process);
        // A stop: dropped only after the browser, told its input is done, ends normally.
        let (mut process, send) = spawn(personal);
        send.send(Ok(Reply::Stopped { id: 2 })).unwrap();
        assert!(
            process.recover().unwrap(),
            "a late stop and normal end drops it"
        );
        drop(process);
        // A creation: only a reply for this browser's own native child, profile
        // cache and pending key makes it usable. Each negative fails exactly one check.
        let (mut process, send) = spawn(work);
        let key = format!("web-home:{work}:https://excalidraw.com");
        process.opening = Some(key.clone());
        let native_pid: u32 = marker(&process.guard_receipt.with_file_name("native"))
            .parse()
            .unwrap();
        let other_pid: u32 = marker(&process.guard_receipt.with_file_name("other"))
            .parse()
            .unwrap();
        let mut stranger = Command::new(&program).arg("30").spawn().unwrap();
        let cache = process.directory.join("cef-profiles").join(work);
        let opened = |id,
                      key: &str,
                      profile: &str,
                      cache: &std::path::Path,
                      browser_id,
                      window_id,
                      native_pid| {
            Ok(Reply::Opened {
                id,
                key: key.to_owned(),
                profile: profile.to_owned(),
                browser_id,
                cache_path: cache.to_string_lossy().into_owned(),
                window_id,
                native_pid,
            })
        };
        let elsewhere = process.directory.join("cef-profiles").join(personal);
        for (case, reply) in [
            ("stale id", opened(1, &key, work, &cache, 1, 7, native_pid)),
            (
                "another key",
                opened(2, "web-home:other", work, &cache, 1, 7, native_pid),
            ),
            (
                "another profile",
                opened(2, &key, personal, &cache, 1, 7, native_pid),
            ),
            (
                "another cache",
                opened(2, &key, work, &elsewhere, 1, 7, native_pid),
            ),
            (
                "no browser",
                opened(2, &key, work, &cache, 0, 7, native_pid),
            ),
            ("no window", opened(2, &key, work, &cache, 1, 0, native_pid)),
            (
                "not the guard's child",
                opened(2, &key, work, &cache, 1, 7, stranger.id()),
            ),
            (
                "another executable",
                opened(2, &key, work, &cache, 1, 7, other_pid),
            ),
        ] {
            send.send(reply).unwrap();
            assert!(!process.recover().unwrap(), "{case}");
            assert!(
                process.uncertain,
                "an unverified creation keeps it in doubt: {case}"
            );
        }
        send.send(opened(2, &key, work, &cache, 1, 7, native_pid))
            .unwrap();
        assert!(
            !process.recover().unwrap(),
            "a verified late creation keeps the browser"
        );
        assert!(!process.uncertain, "and makes it usable again");
        assert_eq!(process.opened["nativePid"], native_pid);
        stranger.kill().unwrap();
        stranger.wait().unwrap();
        drop(process);
        std::fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn a_timed_out_shutdown_leaves_the_late_answer_for_recovery() {
        // The person's cancel is already queued when the reply deadline has
        // passed: the timeout must not consume it.
        let root = std::env::temp_dir().join(format!("sumi-timeout-{}", std::process::id()));
        private_directory(&root).unwrap();
        let work = "11111111-1111-4111-8111-111111111111";
        let guard = recovering_guard(&root);
        let program = std::fs::canonicalize("/bin/sleep").unwrap();
        let parent = spawner().unwrap();
        let (mut process, send) = in_doubt(&root, &guard, &program, &parent, 1, work);
        process.next = 1;
        process.uncertain = false;
        send.send(Ok(Reply::Cancelled {
            id: 2,
            profile: work.to_owned(),
        }))
        .unwrap();
        let passed = std::time::Instant::now()
            .checked_sub(Duration::from_millis(1))
            .unwrap();
        match process.shutdown_until(passed) {
            Err(Error::InvalidRequest(message)) => {
                assert_eq!(message, "Browser response timed out.")
            }
            other => panic!("{other:?}"),
        }
        assert!(process.uncertain, "a timed-out shutdown leaves it in doubt");
        assert!(
            !process.recover().unwrap(),
            "the late cancel keeps the browser"
        );
        assert!(!process.uncertain, "and makes it usable again");
        drop(process);
        std::fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn an_answered_error_settles_the_request_but_a_timeout_does_not() {
        // The browser refused a creation (e.g. its window limit) or a close
        // it could not observe: it holds nothing pending and stays usable.
        let root = std::env::temp_dir().join(format!("sumi-answered-{}", std::process::id()));
        private_directory(&root).unwrap();
        let work = "11111111-1111-4111-8111-111111111111";
        let guard = recovering_guard(&root);
        let program = std::fs::canonicalize("/bin/sleep").unwrap();
        let parent = spawner().unwrap();
        let payload = || {
            serde_json::from_value::<OpenRequest>(serde_json::json!({
                "key":format!("web-home:{work}:https://excalidraw.com"),
                "url":"https://excalidraw.com/","title":"App","profile":work,"webHome":true
            }))
            .unwrap()
        };
        let (mut process, send) = in_doubt(&root, &guard, &program, &parent, 1, work);
        process.next = 1;
        process.uncertain = false;
        send.send(Ok(Reply::Error {
            id: 2,
            message: "Native browser window limit reached".into(),
        }))
        .unwrap();
        match process.open(payload()) {
            Err(Error::Desktop(message)) => {
                assert_eq!(message, "Native browser window limit reached")
            }
            other => panic!("{other:?}"),
        }
        assert!(!process.uncertain, "a refused creation leaves it usable");
        assert_eq!(process.opening, None);
        send.send(Ok(Reply::Error {
            id: 3,
            message: "Native close observation is unavailable; browser retained".into(),
        }))
        .unwrap();
        assert!(process.shutdown().is_err());
        assert!(!process.uncertain, "a refused close leaves it usable");
        // A creation that times out stays in doubt for recovery.
        let passed = std::time::Instant::now()
            .checked_sub(Duration::from_millis(1))
            .unwrap();
        let key = format!("web-home:{work}:https://excalidraw.com");
        assert!(process.expect_open_until(4, &key, passed).is_err());
        assert!(process.uncertain, "a timed-out creation leaves it in doubt");
        assert_eq!(process.opening.as_deref(), Some(key.as_str()));
        // A request that cannot be written leaves it in doubt too.
        process.uncertain = false;
        process.input.take();
        assert!(process.open(payload()).is_err());
        assert!(
            process.uncertain,
            "an unwritten creation leaves it in doubt"
        );
        drop(process);
        std::fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn external_link_requests_never_reach_command_replies() {
        // The stand-in browser asks to open a non-web link and one valid link
        // of its own, then answers the shutdown: the reader consumes both,
        // hands only the valid one to the recording opener (tests never start
        // the desktop's) and forwards neither. Another profile's request is a
        // protocol fault.
        let root = std::env::temp_dir().join(format!("sumi-external-{}", std::process::id()));
        private_directory(&root).unwrap();
        let profile = "11111111-1111-4111-8111-111111111111";
        let marker = format!("own-{}", std::process::id());
        let guard = root.join("guard.py");
        std::fs::write(&guard, format!("import json,sys\nprint(json.dumps({{'event':'openExternal','profile':'{profile}','url':'file:///etc/passwd'}}),flush=True)\nprint(json.dumps({{'event':'openExternal','profile':'{profile}','url':'https://example.com/{marker}'}}),flush=True)\nsys.stdin.readline()\nprint(json.dumps({{'event':'cancelled','id':2,'profile':'{profile}'}}),flush=True)\nsys.stdin.read()\n")).unwrap();
        let parent = spawner().unwrap();
        let payload = serde_json::from_value(serde_json::json!({
            "key":format!("web-home:{profile}:https://excalidraw.com"),
            "url":"https://excalidraw.com/","title":"App","profile":profile,"webHome":true
        }))
        .unwrap();
        let mut process = Process::spawn(
            PathBuf::from("/bin/true"),
            guard,
            root.join("p").join(profile),
            root.join("r"),
            payload,
            &parent,
            true,
        )
        .unwrap();
        process.next = 1;
        assert!(
            !process.shutdown().unwrap(),
            "the cancel arrives after the consumed requests"
        );
        assert!(!process.uncertain);
        let opened = OPENED.lock().unwrap().clone();
        let expected = format!("https://example.com/{marker}");
        // Only this test produces requests, so the record is exactly its own.
        assert_eq!(
            opened,
            vec![expected],
            "only the valid link of the owning profile is opened"
        );
        drop(process);
        let foreign = root.join("foreign.py");
        std::fs::write(&foreign, format!("import json,sys\nprint(json.dumps({{'event':'openExternal','profile':'22222222-2222-4222-8222-222222222222','url':'https://example.com/foreign'}}),flush=True)\nprint(json.dumps({{'event':'openExternal','profile':'{profile}','url':'https://example.com/after-fault'}}),flush=True)\nsys.stdin.read()\n")).unwrap();
        let payload = serde_json::from_value(serde_json::json!({
            "key":format!("web-home:{profile}:https://excalidraw.com"),
            "url":"https://excalidraw.com/","title":"App","profile":profile,"webHome":true
        }))
        .unwrap();
        let mut process = Process::spawn(
            PathBuf::from("/bin/true"),
            foreign,
            root.join("q").join(profile),
            root.join("s"),
            payload,
            &parent,
            true,
        )
        .unwrap();
        process.next = 1;
        match process.shutdown() {
            Err(Error::Desktop(message)) => assert_eq!(message, "Foreign external link request"),
            other => panic!("{other:?}"),
        }
        assert!(
            process.uncertain,
            "a foreign request leaves the browser in doubt"
        );
        let opened = OPENED.lock().unwrap().clone();
        assert!(
            !opened.iter().any(|url| url.ends_with("/foreign")),
            "another profile's link is never opened"
        );
        assert!(
            !opened.iter().any(|url| url.ends_with("/after-fault")),
            "the reader stops at the fault"
        );
        drop(process);
        std::fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn a_closed_reply_channel_ends_a_shutdown_but_a_fault_stays_an_error() {
        // The stand-in guard reads the shutdown request and ends normally
        // without answering, as a browser whose last window just closed.
        let root = std::env::temp_dir().join(format!("sumi-closed-{}", std::process::id()));
        private_directory(&root).unwrap();
        let profile = "11111111-1111-4111-8111-111111111111";
        let receipt = "{'rootExit':0,'complete':True,'kernelNoChildren':True,'remaining':[],'forcedCleanup':0,'error':None,'cleanupErrors':[]}";
        let guard = root.join("guard.py");
        std::fs::write(&guard, format!("import json,pathlib,sys\npathlib.Path(sys.argv[1]).with_name('ready').write_text('1')\nsys.stdin.readline()\nopen(sys.argv[1],'w').write(json.dumps({receipt}))\n")).unwrap();
        let parent = spawner().unwrap();
        for (index, fault) in [false, true].into_iter().enumerate() {
            let payload = serde_json::from_value(serde_json::json!({
                "key":format!("web-home:{profile}:https://excalidraw.com"),
                "url":"https://excalidraw.com/","title":"App","profile":profile,"webHome":true
            }))
            .unwrap();
            let mut process = Process::spawn(
                PathBuf::from("/bin/true"),
                guard.clone(),
                root.join(format!("{index}")).join(profile),
                root.join(format!("r{index}")),
                payload,
                &parent,
                true,
            )
            .unwrap();
            marker(&process.guard_receipt.with_file_name("ready"));
            let (send, replies) = mpsc::channel();
            process.replies = replies;
            if fault {
                send.send(Err("Invalid browser reply".into())).unwrap();
            }
            drop(send);
            match (fault, process.shutdown()) {
                (false, Ok(true)) => {}
                // Even though the guard then ends normally.
                (true, Err(Error::Desktop(message))) => {
                    assert_eq!(message, "Invalid browser reply")
                }
                (_, other) => panic!("fault={fault}: {other:?}"),
            }
            drop(process);
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
    /// A live stand-in browser started through the actual open path, whose
    /// guard runs `script` (with `ready` written beside the receipt position).
    fn stand_in(root: &std::path::Path, script: &str) -> (Process, mpsc::Sender<SpawnRequest>) {
        private_directory(root).unwrap();
        let guard = root.join("guard.py");
        std::fs::write(&guard, script).unwrap();
        let parent = spawner().unwrap();
        let process = Process::spawn(
            PathBuf::from("/bin/true"),
            guard,
            root.join("profile"),
            root.join("r"),
            work_payload(),
            &parent,
            true,
        )
        .unwrap();
        marker(&process.guard_receipt.with_file_name("ready"));
        (process, parent)
    }
    #[test]
    fn a_shutdown_that_cannot_be_written_in_time_does_not_wait_for_a_stop() {
        // The browser never reads its pipe: it never agreed to stop.
        let root = std::env::temp_dir().join(format!("sumi-unread-{}", std::process::id()));
        let (mut process, parent) = stand_in(
            &root,
            "import pathlib,sys,time\npathlib.Path(sys.argv[1]).with_name('ready').write_text('1')\ntime.sleep(30)\n",
        );
        let input = process.input.as_mut().unwrap();
        let block = [b'x'; 4096];
        while input.write(&block).is_ok() {}
        let started = std::time::Instant::now();
        let result =
            process.shutdown_within(Duration::from_millis(100), started + Duration::from_secs(5));
        let elapsed = started.elapsed();
        let alive = process.child.try_wait().unwrap().is_none();
        let uncertain = process.uncertain;
        drop(process);
        std::fs::remove_dir_all(&root).unwrap();
        drop(parent);
        match result {
            Err(Error::Desktop(message)) => assert_eq!(message, "Browser pipe write timed out"),
            other => panic!("{other:?}"),
        }
        assert!(
            elapsed < Duration::from_secs(5),
            "a write timeout returns at once, not after STOP_FINISH: {elapsed:?}"
        );
        assert!(alive && uncertain, "the browser is kept in doubt");
    }
    #[test]
    fn a_shutdown_into_a_closed_pipe_waits_for_the_normal_end() {
        // Its last window just closed: the browser closed its pipe and is
        // still flushing for a moment before its guard ends normally.
        let root = std::env::temp_dir().join(format!("sumi-closedpipe-{}", std::process::id()));
        let (mut process, parent) = stand_in(
            &root,
            &format!("import json,os,pathlib,sys,time\nos.close(0)\npathlib.Path(sys.argv[1]).with_name('ready').write_text('1')\ntime.sleep(1)\nopen(sys.argv[1],'w').write(json.dumps({NORMAL_RECEIPT}))\n"),
        );
        let result = process.shutdown_within(
            Duration::from_millis(100),
            std::time::Instant::now() + Duration::from_secs(5),
        );
        drop(process);
        std::fs::remove_dir_all(&root).unwrap();
        drop(parent);
        assert!(result.unwrap(), "a closed pipe and a normal end stop it");
    }
    #[test]
    fn a_nonreading_child_cannot_hold_the_pipe_writer_past_its_deadline() {
        let mut child = Command::new("sleep")
            .arg("5")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        nonblocking(&input).unwrap();
        let packet=serde_json::to_vec(&Request::Open{id:1,payload:serde_json::from_value(serde_json::json!({
            "key":"web-home:11111111-1111-4111-8111-111111111111:https://excalidraw.com",
            "url":"https://excalidraw.com/","title":"Work","profile":"11111111-1111-4111-8111-111111111111","webHome":true
        })).unwrap()}).unwrap();
        crate::browser_process::decode(&packet, "11111111-1111-4111-8111-111111111111").unwrap();
        let filler = [0u8; 4096];
        loop {
            match input.write(&filler) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                other => panic!("Unexpected pipe state: {other:?}"),
            }
        }
        let start = std::time::Instant::now();
        let result = write_until(&mut input, &packet, start + Duration::from_millis(40));
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
/// Windows: unit tests run by the `cef-windows` CI job, and two tests of the
/// real adapter `Process` with the built `takosumi-browser-guard.exe` and
/// `takosumi-cef-browser-process.exe` (ignored by default; the job runs them
/// with `TAKOSUMI_WINDOWS_GUARD` and `TAKOSUMI_WINDOWS_BROWSER` set).
#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::{os::windows::io::AsRawHandle, path::Path, time::Instant};

    #[test]
    fn an_unresolvable_browser_leaves_the_app_running_with_its_reason() {
        let app = tauri::test::mock_app();
        install(app.handle()).unwrap();
        assert!(app.try_state::<Backend>().is_none());
        let reason = unavailable(app.handle()).expect("an unavailable reason");
        assert!(
            reason.starts_with("The desktop browser is unavailable: ")
                && reason.contains("Browser process executable is unavailable"),
            "{reason}"
        );
        assert!(!super::super::capabilities_of(app.handle()).clear_profile);
    }

    #[test]
    fn a_write_the_browser_does_not_read_times_out_and_holds_the_next() {
        // ping never reads its standard input.
        let mut child = Command::new("ping")
            .args(["-n", "60", "127.0.0.1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut input = windows::Input::new(child.stdin.take().unwrap()).unwrap();
        let started = Instant::now();
        let first = input.write_until(&vec![b'x'; 4 << 20], started + Duration::from_millis(300));
        let second = input.write_until(b"{}\n", Instant::now() + Duration::from_millis(100));
        let elapsed = started.elapsed();
        let passed = input.write_until(b"{}\n", Instant::now());
        child.kill().unwrap();
        child.wait().unwrap();
        // The reader is gone: the pending write fails and the pipe is closed.
        let after = input.write_until(b"{}\n", Instant::now() + Duration::from_secs(10));
        drop(input);
        assert_eq!(first.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
        assert_eq!(second.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
        assert_eq!(passed.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
        assert_eq!(after.unwrap_err().kind(), std::io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn dropping_the_writer_during_a_blocked_write_closes_the_pipe() {
        // The reader sleeps without reading, then reads its input to the end
        // and prints how much arrived. A write that was cancelled when the
        // App's end was dropped leaves part of the 4 MiB, and the end of
        // input follows; a write that went on would deliver all of it.
        // Python is on the windows-latest image (the W1 smoke runs under
        // it too). A PowerShell reader was not used: whether powershell.exe
        // leaves a redirected standard input unread before its -Command
        // script runs was not established.
        let mut child = Command::new("python")
            .args([
                "-I",
                "-c",
                "import sys,time\ntime.sleep(3)\nprint(len(sys.stdin.buffer.read()),flush=True)",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut input = windows::Input::new(child.stdin.take().unwrap()).unwrap();
        let size = 4 << 20;
        let blocked = input.write_until(
            &vec![b'x'; size],
            Instant::now() + Duration::from_millis(300),
        );
        drop(input);
        let mut stdout = child.stdout.take().unwrap();
        let (send, received) = mpsc::channel();
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = std::io::Read::read_to_string(&mut stdout, &mut text);
            let _ = send.send(text);
        });
        let text = received.recv_timeout(Duration::from_secs(30));
        let _ = child.kill();
        child.wait().unwrap();
        assert_eq!(blocked.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
        let delivered: usize = text
            .expect("the reader saw the end of its input in time")
            .trim()
            .parse()
            .unwrap();
        assert!(
            delivered < size,
            "the blocked write was not cancelled: {delivered}"
        );
    }

    #[test]
    fn a_held_chromium_lockfile_refuses_the_clear_and_keeps_the_data() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = std::env::temp_dir().join(format!("sumi-clear-{}", std::process::id()));
        let profile = "11111111-1111-4111-8111-111111111111";
        let cache = root.join(profile).join("cef-profiles");
        private_directory(&cache.join(profile)).unwrap();
        private_directory(&root.join(profile).join("Downloads")).unwrap();
        std::fs::write(cache.join(profile).join("Cookies"), b"x").unwrap();
        std::fs::write(
            root.join(profile).join("Downloads").join("keep.txt"),
            b"keep",
        )
        .unwrap();
        let free = windows::singleton_live(&cache);
        // As Chromium holds it: written, shared for reading only, deleted on close.
        let held = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .share_mode(0x1)
            .custom_flags(0x0400_0000)
            .open(cache.join("lockfile"))
            .unwrap();
        let live = windows::singleton_live(&cache);
        let refused = clear_site_data(&root, profile);
        let kept = cache.join(profile).join("Cookies").exists();
        drop(held);
        let released = !cache.join("lockfile").exists() && !windows::singleton_live(&cache);
        // A leftover file nobody holds is free.
        std::fs::write(cache.join("lockfile"), b"").unwrap();
        let leftover = windows::singleton_live(&cache);
        let cleared = clear_site_data(&root, profile);
        let gone = !cache.exists();
        let downloads = std::fs::read(root.join(profile).join("Downloads").join("keep.txt"));
        std::fs::remove_dir_all(&root).unwrap();
        assert!(!free && live && kept && released && !leftover);
        match refused {
            Err(Error::InvalidRequest(message)) => assert_eq!(message, STILL_OPEN),
            other => panic!("{other:?}"),
        }
        cleared.unwrap();
        assert!(gone);
        assert_eq!(downloads.unwrap(), b"keep");
    }

    /// The built executables, named by the CI job.
    fn built(key: &str) -> PathBuf {
        PathBuf::from(
            std::env::var_os(key).unwrap_or_else(|| panic!("{key} must name the built executable")),
        )
    }

    /// A new case directory in this user's profile (where a runtime root must
    /// be). It is kept for inspection.
    fn case(name: &str) -> PathBuf {
        let base = std::env::var_os("TAKOSUMI_WINDOWS_WORK")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let case = base.join(format!("sumi-w2-{name}-{}-{nanos}", std::process::id()));
        private_directory(&case.join("rt")).unwrap();
        let case = setup::canonical(&case).unwrap();
        println!("case directory: {}", case.display());
        case
    }

    /// The executables and the case's runtime root as `prepare` locates them.
    fn located(case: &Path) -> setup::Located {
        let home = PathBuf::from(std::env::var_os("USERPROFILE").expect("USERPROFILE"));
        let identity = setup::Identity::of(&home).unwrap();
        setup::locate(
            setup::Plan {
                program: built("TAKOSUMI_WINDOWS_BROWSER"),
                guard: built("TAKOSUMI_WINDOWS_GUARD"),
                runtime: setup::Runtime::Given(case.join("rt")),
                // The built executables, as a fixture's overrides name them.
                bundle: None,
            },
            &identity,
        )
        .unwrap()
    }

    fn payload(profile: &str, resume: bool) -> OpenRequest {
        serde_json::from_value(serde_json::json!({
            "key":format!("web-home:{profile}:https://example.com"),
            "url":"https://example.com/","title":"Windows adapter","profile":profile,
            "webHome":true,"resume":resume
        }))
        .unwrap()
    }

    fn tail(path: &Path) -> String {
        let text = std::fs::read(path).unwrap_or_default();
        String::from_utf8_lossy(&text[text.len().saturating_sub(8192)..]).into_owned()
    }

    /// What a failed creation needs for diagnosis: the guard's state, its
    /// children with their images, the expected program and cache path, and
    /// the browser's logs.
    fn report(process: &mut Process, located: &setup::Located, error: &Error) {
        let guard = process.child.id();
        eprintln!("creation failed: {error}");
        eprintln!("guard {guard}: {:?}", process.child.try_wait());
        for pid in windows::tree_of(guard) {
            eprintln!(
                "  process {pid}: parent {:?}, image {:?}",
                windows::parent_of(pid),
                windows::image_of(pid)
            );
        }
        eprintln!("expected program: {:?}", located.program);
        eprintln!(
            "expected cache: {:?}",
            process
                .directory
                .join("cef-profiles")
                .join(&process.profile)
        );
        eprintln!("receipt: {}", tail(&process.guard_receipt));
        logs(process);
    }

    /// The browser's stderr (where a Rust panic message lands before an
    /// abort) and the tail of CEF's own log.
    fn logs(process: &Process) {
        for name in ["browser.log", "cef-native.log"] {
            eprintln!(
                "----- {name} -----\n{}",
                tail(&process.directory.join(name))
            );
        }
    }

    /// Start a profile through the adapter's `Process::spawn` and wait for its
    /// creation reply through `expect_open_until` (what `expect_open` does,
    /// with a cold-start bound of 120 s instead of 20 s; the time taken is
    /// printed).
    fn opened(
        case: &Path,
        located: &setup::Located,
        parent: &mpsc::Sender<SpawnRequest>,
        profile: &str,
    ) -> Process {
        let started = Instant::now();
        let mut process = Process::spawn(
            located.program.clone(),
            located.guard.clone(),
            case.join("profiles").join(profile),
            located.runtime.join(setup::runtime_name(
                std::process::id(),
                NEXT_RUNTIME.fetch_add(1, Ordering::SeqCst),
            )),
            payload(profile, false),
            parent,
            false,
        )
        .expect("the guard starts");
        let key = format!("web-home:{profile}:https://example.com");
        let result = process.expect_open_until(1, &key, started + Duration::from_secs(120));
        println!("opened in {:.3} s", started.elapsed().as_secs_f64());
        if let Err(error) = &result {
            report(&mut process, located, error);
        }
        result.unwrap();
        process
    }

    /// The browser's direct children, each held open before it is examined
    /// (so its id cannot name another process meanwhile): CEF helpers run
    /// the browser's own executable. Only helpers still running when held
    /// count. Waits up to 30 s for the first helper (the page's renderer, if
    /// not already the GPU or a utility process).
    struct Children {
        helpers: Vec<(u32, std::os::windows::io::OwnedHandle)>,
    }
    fn children(native_pid: u32, program: &Path) -> Children {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let mut found = Children {
                helpers: Vec::new(),
            };
            for pid in windows::tree_of(native_pid) {
                let Some(handle) = windows::waitable(pid) else {
                    continue;
                };
                if pid == native_pid || windows::parent_of(pid) != Some(native_pid) {
                    continue;
                }
                if windows::image_of(pid).as_deref() == Some(program)
                    && matches!(crate::windows_job::exit_code(&handle), Ok(None))
                {
                    found.helpers.push((pid, handle));
                }
            }
            if !found.helpers.is_empty() || Instant::now() >= deadline {
                return found;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Every console host (`conhost.exe`) anywhere in `root`'s process tree.
    /// Windows starts one for a console program; the guard is started
    /// detached and the browser and its helpers are a GUI-subsystem program,
    /// so there must be none.
    fn console_hosts(root: u32) -> Vec<u32> {
        windows::tree_of(root)
            .into_iter()
            .filter(|pid| {
                windows::image_of(*pid)
                    .as_deref()
                    .and_then(Path::file_name)
                    .is_some_and(|name| name.eq_ignore_ascii_case("conhost.exe"))
            })
            .collect()
    }

    /// A fresh view of `root`'s process tree for a failure message: each
    /// process with its recorded parent and image, and the expected program.
    fn describe(root: u32, program: &Path) -> String {
        let mut lines = vec![format!("expected program: {program:?}")];
        for pid in windows::tree_of(root) {
            lines.push(format!(
                "  process {pid}: parent {:?}, image {:?}",
                windows::parent_of(pid),
                windows::image_of(pid)
            ));
        }
        lines.join("\n")
    }

    /// The entries of a directory, for a failure message.
    fn listing(directory: &Path) -> Vec<String> {
        std::fs::read_dir(directory)
            .map(|entries| {
                entries
                    .filter_map(|entry| {
                        Some(entry.ok()?.file_name().to_string_lossy().into_owned())
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    #[ignore = "needs the built Windows guard and browser (see the module comment)"]
    fn windows_adapter_opens_reuses_and_shuts_down_a_profile() {
        let case = case("open");
        let located = located(&case);
        let parent = spawner().unwrap();
        let profile = "11111111-1111-4111-8111-111111111111";
        let mut process = opened(&case, &located, &parent, profile);
        let first = process.opened.clone();
        let native_pid = first["nativePid"].as_u64().unwrap() as u32;
        // What confirm_opened accepted, restated against Windows' records.
        assert_ne!(native_pid, process.child.id());
        assert_eq!(windows::parent_of(native_pid), Some(process.child.id()));
        assert_eq!(
            windows::image_of(native_pid).as_deref(),
            Some(located.program.as_path())
        );
        assert_eq!(
            first["nativeCache"].as_str(),
            process
                .directory
                .join("cef-profiles")
                .join(profile)
                .to_str()
        );
        let tree = windows::tree_of(process.child.id());
        let children = children(native_pid, &located.program);
        let ids = |held: &[(u32, _)]| held.iter().map(|(pid, _)| *pid).collect::<Vec<_>>();
        let hosts = console_hosts(process.child.id());
        println!(
            "guard tree while open: {tree:?}; CEF helpers: {:?}; console hosts: {hosts:?}",
            ids(&children.helpers)
        );
        assert!(
            !children.helpers.is_empty(),
            "no CEF helper below the browser:\n{}",
            describe(native_pid, &located.program)
        );
        // No console host anywhere below the guard: the guard is detached,
        // and the browser and its helpers are a GUI-subsystem program.
        assert!(
            hosts.is_empty(),
            "console host(s) {hosts:?} in the guard's tree (a console program?):\n{}",
            describe(process.child.id(), &located.program)
        );
        // CEF's process singleton holds the profile's lockfile while the
        // browser runs: what clear_profile's refusal relies on.
        let root = process.directory.join("cef-profiles");
        assert!(
            windows::singleton_live(&root),
            "the live profile's lockfile is not held: {:?}",
            listing(&root)
        );
        // Reopening the same screen reuses its native browser and window.
        let reopened = process.open(payload(profile, true));
        if reopened.is_err() {
            logs(&process);
        }
        reopened.expect("the same screen reopens");
        for field in ["browserId", "windowId", "nativePid", "nativeCache"] {
            assert_eq!(process.opened[field], first[field], "{field}");
        }
        // The adapter's ordinary shutdown: the browser stops and its guard
        // ends normally with a complete receipt.
        let stopping = Instant::now();
        let stopped = process.shutdown();
        let receipt = std::fs::read(&process.guard_receipt)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
        println!(
            "{}",
            serde_json::json!({
                "opened": first,
                "shutdownSeconds": stopping.elapsed().as_secs_f64(),
                "guardExit": format!("{:?}", process.child.try_wait()),
                "receipt": receipt,
            })
        );
        if !stopped.as_ref().is_ok_and(|stopped| *stopped)
            || receipt
                .as_ref()
                .is_none_or(|receipt| require_normal_guard(receipt).is_err())
        {
            logs(&process);
        }
        assert!(
            stopped.expect("the profile shuts down"),
            "no page cancelled"
        );
        let receipt = receipt.expect("a guard receipt");
        require_normal_guard(&receipt).unwrap();
        assert_eq!(
            receipt["kernelNoChildren"], true,
            "the guard's job is empty"
        );
        // The browser and the helpers held while it ran were all in the
        // guard's job: a helper outside it would leave the count short.
        let expected = 1 + children.helpers.len();
        let observed = receipt["observed"].as_u64().unwrap();
        assert!(
            observed >= expected as u64,
            "the guard's job counted {observed} processes, fewer than the browser and \
             {} CEF helper(s)",
            children.helpers.len()
        );
        assert!(process
            .child
            .try_wait()
            .unwrap()
            .is_some_and(|status| status.success()));
        assert!(
            !windows::singleton_live(&root),
            "the ended profile's lockfile is released: {:?}",
            listing(&root)
        );
        let runtime = process.runtime.clone();
        drop(process);
        assert!(
            !runtime.exists(),
            "an ended browser's runtime directory goes"
        );
        drop(parent);
    }

    #[test]
    #[ignore = "needs the built Windows guard and browser (see the module comment)"]
    fn windows_adapter_ending_the_app_job_ends_guard_and_browser() {
        let case = case("job");
        let located = located(&case);
        let parent = spawner().unwrap();
        let profile = "22222222-2222-4222-8222-222222222222";
        let mut process = opened(&case, &located, &parent, profile);
        let native_pid = process.opened["nativePid"].as_u64().unwrap() as u32;
        // Helpers held while running: one that ends by itself before the
        // App's job does has still ended, so it cannot make this flaky.
        let children = children(native_pid, &located.program);
        assert!(
            !children.helpers.is_empty(),
            "no CEF helper below the browser:\n{}",
            describe(native_pid, &located.program)
        );
        let tree = windows::tree_of(process.child.id());
        println!(
            "guard tree before the App's job ends: {tree:?}; CEF helpers: {:?}",
            children
                .helpers
                .iter()
                .map(|(pid, _)| *pid)
                .collect::<Vec<_>>()
        );
        assert!(tree.contains(&native_pid), "{tree:?}");
        // The guard and the browser itself must still be there.
        let mut handles = tree
            .iter()
            .filter_map(|pid| windows::waitable(*pid).map(|handle| (*pid, handle)))
            .collect::<Vec<_>>();
        for pid in [process.child.id(), native_pid] {
            assert!(
                handles.iter().any(|(held, _)| *held == pid),
                "process {pid} is not open"
            );
        }
        handles.extend(children.helpers);
        // The App side ends: its spawner closes the App's Job Object, as the
        // App process's end does.
        drop(parent);
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut outlived = Vec::new();
        for (pid, handle) in &handles {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let ended = crate::windows_job::wait_any(&[handle.as_raw_handle()], Some(remaining));
            if !matches!(ended, Ok(Some(0))) {
                outlived.push(format!("{pid} {:?} {ended:?}", windows::image_of(*pid)));
            }
        }
        assert!(
            outlived.is_empty(),
            "processes outlived the App's job: {outlived:?}"
        );
        assert!(process.child.try_wait().unwrap().is_some());
        assert!(
            process.normal_guard().is_err(),
            "a guard ended with the App leaves no normal receipt"
        );
        drop(process);
    }
}
/// The fixture guard (`guard.py`) runs under python3; the product guard
/// (`takosumi-browser-guard`) is executed directly. Both take the same
/// arguments and write the same receipt.
fn guard_command(guard: &std::path::Path) -> Command {
    if guard.extension() == Some(std::ffi::OsStr::new("py")) {
        let mut command = Command::new("python3");
        command.arg("-B").arg(guard);
        command
    } else {
        Command::new(guard)
    }
}
/// The Windows environment a guard and its browser keep: the system
/// locations Windows components and Chromium read (`SystemRoot` above all;
/// Winsock and other system libraries fail without it), the user's profile
/// folders and `PATH`. `TEMP` and `TMP` point into the browser's runtime
/// directory instead, and the guard's stop event is added. Nothing else of
/// the App's environment passes.
#[cfg(windows)]
const WINDOWS_ENVIRONMENT: &[&str] = &[
    "SystemRoot",
    "SystemDrive",
    "windir",
    "ComSpec",
    "PATH",
    "PATHEXT",
    "USERPROFILE",
    "LOCALAPPDATA",
    "APPDATA",
    "ProgramData",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "CommonProgramFiles",
    "ALLUSERSPROFILE",
    "PUBLIC",
    "USERNAME",
    "COMPUTERNAME",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
];
fn spawn_from_parent(spawner: &mpsc::Sender<SpawnRequest>, command: Command) -> Result<Child> {
    let (reply, spawned) = mpsc::sync_channel(1);
    spawner
        .send((command, reply))
        .map_err(|_| Error::InvalidRequest("Browser process parent is unavailable."))?;
    spawned.recv().map_err(failure)?.map_err(failure)
}
#[cfg(target_os = "linux")]
fn write_input(
    input: &mut Input,
    bytes: &[u8],
    deadline: std::time::Instant,
) -> std::io::Result<()> {
    write_until(input, bytes, deadline)
}
#[cfg(windows)]
fn write_input(
    input: &mut Input,
    bytes: &[u8],
    deadline: std::time::Instant,
) -> std::io::Result<()> {
    input.write_until(bytes, deadline)
}
#[cfg(target_os = "linux")]
fn nonblocking(input: &ChildStdin) -> Result<()> {
    // SAFETY: this owned pipe is live and protected by the backend mutex.
    let flags = unsafe { libc::fcntl(input.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(input.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(failure(std::io::Error::last_os_error()));
    }
    Ok(())
}
/// The `opened` reply's native process is the guard's own child running
/// exactly `program` (Windows: `process_windows::native_owned`).
#[cfg(target_os = "linux")]
fn native_owned(native_pid: u32, guard: &Child, program: &std::path::Path) -> bool {
    native_parent(native_pid) == Some(guard.id())
        && std::fs::read_link(format!("/proc/{native_pid}/exe"))
            .ok()
            .as_deref()
            == Some(program)
}
/// The cache path CEF reported is the requested one (Windows compares the
/// spelling exactly: `process_windows::same_path`).
#[cfg(target_os = "linux")]
fn same_path(reported: &str, expected: &std::path::Path) -> bool {
    std::path::Path::new(reported) == expected
}
#[cfg(windows)]
use windows::{native_owned, same_path, singleton_live};
#[cfg(target_os = "linux")]
fn native_parent(pid: u32) -> Option<u32> {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()?
        .rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}
/// A runtime directory created for a guard that has not started yet; removed
/// if spawning fails before a `Process` owns it.
struct Unowned(Option<PathBuf>);
impl Drop for Unowned {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            if let Err(problem) = setup::remove_runtime(&runtime) {
                eprintln!("A browser runtime directory was not removed: {problem}");
            }
        }
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.input.take();
        if matches!(self.child.try_wait(), Ok(None)) {
            // A live, unreaped direct Child cannot have its PID recycled.
            // SIGTERM asks the dedicated pidfd/subreaper guard to drain all
            // owned descendants; never SIGKILL that guard or scan by name.
            #[cfg(target_os = "linux")]
            unsafe {
                libc::kill(self.child.id() as i32, libc::SIGTERM);
            }
            // Windows: the guard's stop event. Never terminate the guard: its
            // job would end the browser without a drain or a receipt.
            #[cfg(windows)]
            if let Err(error) = self.stop.set() {
                eprintln!("A browser guard could not be asked to stop: {error}");
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while std::time::Instant::now() < deadline {
                if !matches!(self.child.try_wait(), Ok(None)) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        // Every caller drops a browser right after verifying its guard's
        // normal receipt; any other ended guard is dropped only at the App's
        // end. A guard still running keeps its sockets and receipt.
        if !setup::KEEP_ENDED_RUNTIME && matches!(self.child.try_wait(), Ok(Some(_))) {
            if let Err(problem) = setup::remove_runtime(&self.runtime) {
                eprintln!("A browser runtime directory was not removed: {problem}");
            }
        }
    }
}
