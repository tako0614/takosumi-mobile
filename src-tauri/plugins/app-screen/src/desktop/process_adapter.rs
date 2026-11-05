//! Explicit Linux experiment behind the existing shell-only `open` command.
use super::{failure, private_directory};
use crate::{
    browser_process::{read_packet, write_until, Reply, Request, MAX_PACKET_BYTES},
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
struct Process {
    child: Child,
    input: Option<ChildStdin>,
    replies: mpsc::Receiver<std::result::Result<Reply, String>>,
    profile: String,
    directory: PathBuf,
    next: u64,
    uncertain: bool,
    opened: serde_json::Value,
    guard_receipt: PathBuf,
    program: PathBuf,
    /// Opening order: an App quit closes the oldest profile first.
    order: u64,
}
static NEXT_ORDER: AtomicU64 = AtomicU64::new(1);

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
            process.uncertain = result.is_err();
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
            Err(error) => {
                process.uncertain = true;
                return Err(error);
            }
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
        let mut command = Command::new("python3");
        command
            .args(["-B"])
            .arg(guard)
            .arg(&guard_receipt)
            .arg(&program);
        // The fixture guard owns this process group and its exact descendants.
        // Do not put it in the App's or another runner's process group.
        command.process_group(0);
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
            opened: serde_json::Value::Null,
            guard_receipt,
            program,
            order: NEXT_ORDER.fetch_add(1, Ordering::SeqCst),
        };
        nonblocking(process.input.as_ref().unwrap())?;
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
    fn receive(&mut self, id: u64, deadline: std::time::Instant) -> Result<Reply> {
        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .ok_or(Error::InvalidRequest("Browser response timed out."))?;
            let reply = self
                .replies
                .recv_timeout(remaining)
                .map_err(failure)?
                .map_err(failure)?;
            if let Reply::Closed { id: 0, profile, .. } = &reply {
                if profile != &self.profile {
                    return Err(Error::InvalidRequest("Foreign browser close reply."));
                }
                continue;
            }
            let actual = match &reply {
                Reply::Opened { id, .. }
                | Reply::Closed { id, .. }
                | Reply::Stopped { id }
                | Reply::Error { id, .. }
                | Reply::CloseBlocked { id, .. }
                | Reply::Cancelled { id, .. } => *id,
            };
            if actual != id {
                return Err(Error::InvalidRequest("Browser response identity mismatch."));
            }
            if let Reply::Error { message, .. } = reply {
                return Err(failure(message));
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
        match self.receive(id, deadline)? {
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
        let result = self
            .send(Request::Open { id, payload }, deadline)
            .and_then(|()| self.expect_open_until(id, &key, deadline));
        self.uncertain = result.is_err();
        result
    }
    fn shutdown(&mut self) -> Result<bool> {
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
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let reply = match self
            .send(Request::Shutdown { id }, deadline)
            .and_then(|()| self.receive(id, deadline))
        {
            Ok(reply) => reply,
            // A browser whose last window was just closed is already ending:
            // its pipe or replies close first. Accept only a normal end.
            Err(error) => {
                return match self.ended_within(Duration::from_secs(2)) {
                    Ok(()) => Ok(true),
                    Err(_) => Err(error),
                }
            }
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
        std::fs::write(&exiting, "import json,sys,time\ntime.sleep(0.3)\nopen(sys.argv[1],'w').write(json.dumps({'rootExit':0,'complete':True,'kernelNoChildren':True,'remaining':[],'forcedCleanup':0,'error':None,'cleanupErrors':[]}))\n").unwrap();
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
