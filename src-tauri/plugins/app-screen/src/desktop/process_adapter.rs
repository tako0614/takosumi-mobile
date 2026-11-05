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
    // This CEF interface cannot prepare/roll back a cross-process beforeunload
    // transaction. Refuse before sending any close, rather than destroy one
    // profile before a later profile can cancel. Users can close native
    // browser windows individually; coordinated App quit remains unsupported.
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
    if state.processes.len() > 1 {
        return Err(Error::InvalidRequest(
            "Coordinated browser quit is unavailable; close browser windows individually first.",
        ));
    }
    let profiles = state.processes.keys().cloned().collect::<Vec<_>>();
    for profile in profiles {
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
        let mut child = command.spawn().map_err(failure)?;
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
        self.send(Request::Shutdown { id }, deadline)?;
        match self.receive(id, deadline)? {
            Reply::Cancelled { profile, .. } if profile == self.profile => return Ok(false),
            Reply::CloseBlocked { profile, .. } if profile == self.profile => {
                eprintln!(
                    "Coordinated window quit unavailable; close native windows individually first."
                );
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
