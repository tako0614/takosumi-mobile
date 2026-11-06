//! Explicit Linux experiment behind the existing shell-only `open` command.
use super::{failure, private_directory};
use crate::{
    browser_process::{external_link, read_packet, write_until, Reply, Request, MAX_PACKET_BYTES},
    Error, OpenRequest, Result,
};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{BufReader, Write},
    os::fd::AsRawFd,
    os::unix::{fs::OpenOptionsExt, process::CommandExt},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicI32, AtomicU64, AtomicU8, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
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
    directory: PathBuf,
    runtime_root: PathBuf,
    processes: BTreeMap<String, Process>,
    spawner: mpsc::Sender<SpawnRequest>,
}
type SpawnRequest = (Command, mpsc::SyncSender<std::io::Result<Child>>);

/// Linux delivers a child's parent-death signal when the *thread* that forked
/// it exits, not only when the App process exits. Tauri's blocking pool
/// retires idle threads, which would SIGTERM every guard forked from one. Fork
/// guards only from this thread, which stays alive for the whole App process.
/// If it ever panicked, its guards drain their browsers and later opens fail
/// as unavailable: closed, never an unowned browser. Requesters always wait
/// for the reply, so a spawned guard is never left untracked.
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
    input: Option<ChildStdin>,
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
    program: PathBuf,
    /// Opening order: an App quit closes the oldest profile first.
    order: u64,
}
static NEXT_ORDER: AtomicU64 = AtomicU64::new(1);
/// Openers started and not yet ended; a burst beyond this is dropped.
#[cfg(not(test))]
static OPENERS: AtomicU64 = AtomicU64::new(0);
/// Hand a validated link to the desktop's opener without waiting for it. Its
/// own process group keeps the desktop browser it may start out of the App's
/// terminal signals.
#[cfg(not(test))]
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
/// Tests never start the desktop's opener: they record what would be opened.
#[cfg(test)]
static OPENED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
#[cfg(test)]
fn open_externally(url: &str) {
    OPENED.lock().unwrap().push(url.to_owned());
}

pub(super) fn install(app: &AppHandle) -> Result<()> {
    let program = PathBuf::from(std::env::var_os("TAKOSUMI_BROWSER_PROCESS_PROGRAM").ok_or(
        Error::InvalidRequest("The explicit browser-process fixture needs its executable."),
    )?)
    .canonicalize()
    .map_err(failure)?;
    if !program.is_file() {
        return Err(Error::InvalidRequest(
            "Browser process executable is unavailable.",
        ));
    }
    let guard = PathBuf::from(std::env::var_os("TAKOSUMI_BROWSER_PROCESS_GUARD").ok_or(
        Error::InvalidRequest("The browser fixture needs its owned descendant guard."),
    )?)
    .canonicalize()
    .map_err(failure)?;
    if !guard.is_file() {
        return Err(Error::InvalidRequest(
            "Browser fixture guard is unavailable.",
        ));
    }
    let directory = app
        .path()
        .app_local_data_dir()
        .map_err(failure)?
        .join("browser-process-experiment");
    private_directory(&directory)?;
    let directory = directory.canonicalize().map_err(failure)?;
    let runtime_root = PathBuf::from(std::env::var_os("TAKOSUMI_BROWSER_PROCESS_RUNTIME").ok_or(
        Error::InvalidRequest("The browser fixture needs an owned short runtime directory."),
    )?)
    .canonicalize()
    .map_err(failure)?;
    if !runtime_root.is_dir() {
        return Err(Error::InvalidRequest(
            "Browser runtime directory is unavailable.",
        ));
    }
    app.manage(Backend(
        Arc::new(Mutex::new(State {
            program,
            guard,
            directory,
            runtime_root,
            processes: BTreeMap::new(),
            spawner: spawner()?,
        })),
        Arc::new(ExitPhase::default()),
    ));
    Ok(())
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
            let runtime = state.runtime_root.join(format!(
                "p-{}-{}",
                std::process::id(),
                NEXT_RUNTIME.fetch_add(1, Ordering::SeqCst)
            ));
            let mut process = Process::spawn(
                state.program.clone(),
                state.guard.clone(),
                state.directory.join(&profile),
                runtime,
                payload,
                &state.spawner,
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
    let backend = app.state::<Backend>().inner().clone();
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
    let backend = app.state::<Backend>();
    let state = backend.0.lock().map_err(failure)?;
    Ok(serde_json::Value::Array(state.processes.values().map(|process|serde_json::json!({
        "pid":process.child.id(),"profile":process.profile,"directory":process.directory,"guardReceipt":process.guard_receipt,
        "next":process.next,"uncertain":process.uncertain,"opened":process.opened,
        "closing":backend.1.state.load(Ordering::Acquire),"exitRequests":backend.1.requests.load(Ordering::Acquire)
    })).collect()))
}

impl Process {
    fn spawn(
        program: PathBuf,
        guard: PathBuf,
        directory: PathBuf,
        runtime: PathBuf,
        payload: OpenRequest,
        spawner: &mpsc::Sender<SpawnRequest>,
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
        let directory = directory.canonicalize().map_err(failure)?;
        // An interrupted clear may have left a detached old cache root.
        if let Err(problem) = remove_detached(&directory) {
            eprintln!("Detached browser data was not removed: {problem}");
        }
        let profile = payload.profile.clone();
        if runtime
            .join("tmp/org.chromium.Chromium.XXXXXX/SingletonSocket")
            .as_os_str()
            .len()
            >= 108
        {
            return Err(Error::InvalidRequest(
                "Browser runtime socket path is too long.",
            ));
        }
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&runtime)
            .map_err(failure)?;
        let boot = Request::Open { id: 1, payload };
        boot.validate(&profile)?;
        let bytes = serde_json::to_vec(&boot).map_err(failure)?;
        if bytes.len() > MAX_PACKET_BYTES {
            return Err(Error::InvalidRequest("Browser request is too large."));
        }
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(directory.join("boot.json"))
            .map_err(failure)?;
        file.write_all(&bytes).map_err(failure)?;
        file.flush().map_err(failure)?;
        let guard_receipt = runtime.join("guard.json");
        let mut command = guard_command(&guard);
        command.arg(&guard_receipt).arg(&program);
        // The guard owns this process group and its exact descendants. Do not
        // put it in the App's or another runner's process group.
        command.process_group(0);
        // Tie the guard to the App from the moment it forks: if the App died
        // before this point, the guard would otherwise arm its parent-death
        // signal against its new parent and outlive the App.
        let app = std::process::id();
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0) != 0 {
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
        if std::env::var_os("TAKOSUMI_BROWSER_PROCESS_INSPECT").as_deref()
            == Some(std::ffi::OsStr::new("1"))
        {
            command.env("TAKOSUMI_BROWSER_PROCESS_INSPECT", "1");
        }
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
        private_directory(&runtime.join("tmp"))?;
        command.env("TMPDIR", runtime.join("tmp"));
        private_directory(&directory.join("Downloads"))?;
        let output = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(directory.join("browser.log"))
            .map_err(failure)?;
        command
            .env("TAKOSUMI_BROWSER_PROCESS_DIRECTORY", &directory)
            .env("LD_LIBRARY_PATH", program.parent().unwrap())
            .args([
                "--no-sandbox",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-networking",
                "--disable-component-update",
                "--disable-sync",
                "--password-store=basic",
                "--lang=en-US",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(output);
        let mut child = spawn_from_parent(spawner, command)?;
        let input = child.stdin.take().expect("Owned child stdin");
        let stdout = child.stdout.take().expect("Owned child stdout");
        let (send, replies) = mpsc::channel();
        let process = Self {
            child,
            input: Some(input),
            replies,
            profile,
            directory,
            next: 1,
            uncertain: false,
            opening: None,
            opened: serde_json::Value::Null,
            guard_receipt,
            program,
            order: NEXT_ORDER.fetch_add(1, Ordering::SeqCst),
        };
        nonblocking(process.input.as_ref().unwrap())?;
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
        request.validate(&self.profile)?;
        let mut bytes = serde_json::to_vec(&request).map_err(failure)?;
        if bytes.len() > MAX_PACKET_BYTES {
            return Err(Error::InvalidRequest("Browser request is too large."));
        }
        bytes.push(b'\n');
        let input = self
            .input
            .as_mut()
            .ok_or(Error::InvalidRequest("Browser pipe is closed."))?;
        write_until(input, &bytes, deadline).map_err(failure)
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
                && native_parent(native_pid) == Some(self.child.id())
                && std::fs::read_link(format!("/proc/{native_pid}/exe")).ok()
                    == Some(self.program.clone())
                && PathBuf::from(&cache_path)
                    == self.directory.join("cef-profiles").join(&self.profile) =>
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
        let mut rejected = false;
        let result = self.shutdown_attempt(reply_deadline, &mut rejected);
        if result.is_err() && !rejected {
            self.uncertain = true;
        }
        result
    }
    fn shutdown_attempt(
        &mut self,
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
        if let Err(error) = self.send(Request::Shutdown { id }, now + Duration::from_secs(20)) {
            return self
                .ended_within(Duration::from_secs(2))
                .map(|()| true)
                .map_err(|_| error);
        }
        // A timeout leaves any late answer queued for `recover`.
        let reply = match self.receive(id, reply_deadline) {
            Ok(reply) => reply,
            Err(Wait::Closed) => {
                return self
                    .ended_within(Duration::from_secs(2))
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
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
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
                    return self.ended_within(Duration::from_secs(5)).map(|()| true);
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

#[cfg(test)]
mod tests {
    use super::*;
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
    /// beside the receipt position, then waits for the control pipe to close.
    const DIRECT_GUARD: &str = "#!/bin/sh\nprintf '%s\\n' \"$0\" \"$@\" > \"${1%/*}/argv\"\n: > \"${1%/*}/ready\"\nwhile read -r line; do :; done\n";
    #[test]
    fn open_requests_execute_a_guard_binary_directly() {
        let root = std::env::temp_dir().join(format!("sumi-pa-direct-{}", std::process::id()));
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
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !runtime.join("ready").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let recorded = std::fs::read_to_string(runtime.join("argv"));
        drop(process); // Closing the control pipe ends the stand-in.
        std::fs::remove_dir_all(&root).unwrap();
        let recorded = recorded.expect("the guard binary was not executed directly");
        let lines = recorded.lines().collect::<Vec<_>>();
        let receipt = runtime.join("guard.json");
        assert_eq!(
            lines[..3],
            [
                guard.to_str().unwrap(),
                receipt.to_str().unwrap(),
                "/bin/true"
            ]
        );
        assert!(lines.contains(&"--no-sandbox"), "{lines:?}");
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
            )
            .unwrap();
            processes.insert(profile.to_owned(), process);
        }
        let mut state = State {
            program: PathBuf::from("/bin/true"),
            guard: lingering.clone(),
            directory: root.clone(),
            runtime_root: root.clone(),
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
fn spawn_from_parent(spawner: &mpsc::Sender<SpawnRequest>, command: Command) -> Result<Child> {
    let (reply, spawned) = mpsc::sync_channel(1);
    spawner
        .send((command, reply))
        .map_err(|_| Error::InvalidRequest("Browser process parent is unavailable."))?;
    spawned.recv().map_err(failure)?.map_err(failure)
}
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
impl Drop for Process {
    fn drop(&mut self) {
        self.input.take();
        if matches!(self.child.try_wait(), Ok(None)) {
            // A live, unreaped direct Child cannot have its PID recycled.
            // SIGTERM asks the dedicated pidfd/subreaper guard to drain all
            // owned descendants; never SIGKILL that guard or scan by name.
            unsafe {
                libc::kill(self.child.id() as i32, libc::SIGTERM);
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while std::time::Instant::now() < deadline {
                if !matches!(self.child.try_wait(), Ok(None)) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}
