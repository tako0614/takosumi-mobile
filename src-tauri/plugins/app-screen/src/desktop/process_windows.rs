//! Windows pieces of the per-profile browser adapter (`process_adapter.rs`):
//! the bounded control-pipe writer, the identity checks on a browser's
//! `opened` reply, the guard's stop event name, Chromium's profile lock and
//! the external-link opener. Minimal `extern "system"` declarations of
//! documented Win32 functions; no crate dependency.
use std::{
    ffi::{c_void, OsStr, OsString},
    io::{self, Write},
    os::windows::{
        ffi::OsStringExt,
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle},
    },
    path::{Path, PathBuf},
    process::{Child, ChildStdin},
    sync::{mpsc, Arc},
    thread::JoinHandle,
    time::Instant,
};

type Handle = *mut c_void;

const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const TH32CS_SNAPPROCESS: u32 = 0x2;
const INVALID_HANDLE_VALUE: isize = -1;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
#[cfg(not(test))]
const COINIT_APARTMENTTHREADED: u32 = 0x2;
#[cfg(not(test))]
const COINIT_DISABLE_OLE1DDE: u32 = 0x4;
#[cfg(not(test))]
const SW_SHOWNORMAL: i32 = 1;
#[cfg(test)]
const SYNCHRONIZE: u32 = 0x0010_0000;

#[repr(C)]
struct ProcessEntry {
    // PROCESSENTRY32W
    size: u32,
    usage: u32,
    pid: u32,
    heap: usize,
    module: u32,
    threads: u32,
    parent: u32,
    priority: i32,
    flags: u32,
    exe: [u16; 260],
}
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<ProcessEntry>() == 568);

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn QueryFullProcessImageNameW(
        process: Handle,
        flags: u32,
        name: *mut u16,
        size: *mut u32,
    ) -> i32;
    fn GetProcessTimes(
        process: Handle,
        creation: *mut u64,
        exit: *mut u64,
        kernel: *mut u64,
        user: *mut u64,
    ) -> i32;
    fn CreateToolhelp32Snapshot(flags: u32, process: u32) -> Handle;
    fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry) -> i32;
    fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry) -> i32;
    fn CancelIoEx(file: Handle, overlapped: *mut c_void) -> i32;
}
#[cfg(not(test))]
#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> i32;
    fn CoUninitialize();
}
#[cfg(not(test))]
#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        window: Handle,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show: i32,
    ) -> Handle;
}

fn owned(handle: Handle) -> Option<OwnedHandle> {
    if handle.is_null() || handle as isize == INVALID_HANDLE_VALUE {
        return None;
    }
    // SAFETY: a fresh handle this process now owns alone.
    Some(unsafe { OwnedHandle::from_raw_handle(handle as RawHandle) })
}

#[cfg(not(test))]
fn wide(text: &OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    text.encode_wide().chain(Some(0)).collect()
}

/// (process id, recorded parent id) of every process, from one snapshot.
fn processes() -> Vec<(u32, u32)> {
    let mut found = Vec::new();
    // SAFETY: a snapshot of every process; the result is checked.
    let Some(snapshot) = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return found;
    };
    let snapshot: Handle = snapshot.as_raw_handle().cast();
    // SAFETY: PROCESSENTRY32W is plain data.
    let mut entry: ProcessEntry = unsafe { std::mem::zeroed() };
    entry.size = std::mem::size_of::<ProcessEntry>() as u32;
    // SAFETY: a live snapshot and an entry with its size set.
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more {
        found.push((entry.pid, entry.parent));
        entry.size = std::mem::size_of::<ProcessEntry>() as u32;
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    found
}

/// The parent process id Windows recorded for `pid`, from a process
/// snapshot. A recorded parent may have ended and its id been reused, so
/// `native_owned` also compares creation times.
pub(super) fn parent_of(pid: u32) -> Option<u32> {
    processes()
        .into_iter()
        .find(|(id, _)| *id == pid)
        .map(|(_, parent)| parent)
}

/// `root` and every process below it by recorded parents, created no
/// earlier than `root` (so no record left by a reused id is followed).
#[cfg(test)]
pub(super) fn tree_of(root: u32) -> Vec<u32> {
    let all = processes();
    let born = |pid| query(pid).and_then(|process| created(process.as_raw_handle().cast()));
    let Some(root_born) = born(root) else {
        return Vec::new();
    };
    let mut tree = vec![root];
    let mut next = 0;
    while next < tree.len() {
        let parent = tree[next];
        for (pid, recorded) in &all {
            if *recorded == parent
                && !tree.contains(pid)
                && born(*pid).is_some_and(|born| born >= root_born)
            {
                tree.push(*pid);
            }
        }
        next += 1;
    }
    tree
}

/// A handle to wait for a process's end.
#[cfg(test)]
pub(super) fn waitable(pid: u32) -> Option<OwnedHandle> {
    // SAFETY: plain call; the result is checked.
    owned(unsafe { OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })
}

fn query(pid: u32) -> Option<OwnedHandle> {
    // SAFETY: plain call; the result is checked.
    owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })
}

/// The full Win32 path of a process's executable image.
pub(super) fn image_of(pid: u32) -> Option<PathBuf> {
    let process = query(pid)?;
    let mut buffer = vec![0u16; 32_768];
    let mut size = buffer.len() as u32;
    // SAFETY: a live handle and a buffer of `size` UTF-16 units.
    if unsafe {
        QueryFullProcessImageNameW(
            process.as_raw_handle().cast(),
            0,
            buffer.as_mut_ptr(),
            &mut size,
        )
    } == 0
    {
        return None;
    }
    Some(PathBuf::from(OsString::from_wide(&buffer[..size as usize])))
}

fn created(process: Handle) -> Option<u64> {
    let (mut creation, mut exit, mut kernel, mut user) = (0, 0, 0, 0);
    // SAFETY: a live process handle; FILETIME is two little-endian u32 halves.
    (unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) } != 0)
        .then_some(creation)
}

/// Whether the `opened` reply's native process is the guard's own child
/// running exactly `program`: Windows records the guard as its parent, it
/// was created after the guard (so the record is not one left by an ended
/// process whose id the guard reused), and its image path is `program`,
/// compared exactly. The guard is unreaped, so its id names it.
pub(super) fn native_owned(native_pid: u32, guard: &Child, program: &Path) -> bool {
    if parent_of(native_pid) != Some(guard.id()) {
        return false;
    }
    let Some(native) = query(native_pid) else {
        return false;
    };
    let (Some(born), Some(guard_born)) = (
        created(native.as_raw_handle().cast()),
        created(guard.as_raw_handle().cast()),
    ) else {
        return false;
    };
    born >= guard_born
        && image_of(native_pid).as_deref().map(Path::as_os_str) == Some(program.as_os_str())
}

/// The cache path the browser reported, compared exactly with the one the
/// App requested (`Path` equality would accept `/` or another letter case).
pub(super) fn same_path(reported: &str, expected: &Path) -> bool {
    OsStr::new(reported) == expected.as_os_str()
}

/// A fresh name for one guard's stop event, in this session's namespace.
pub(super) fn stop_event_name(runtime: &Path) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let name = runtime
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("runtime");
    format!(
        r"Local\takosumi-browser-guard-stop-{}-{name}-{nanos}",
        std::process::id()
    )
}

/// Chromium's process singleton on Windows holds `lockfile` in the profile's
/// user data directory (CEF's root cache path) open for writing, shared for
/// reading only, and deletes it on close. Any browser that still has the
/// profile, tracked by this App or not, makes an exclusive open fail. A
/// missing file, or one nobody holds, is free; any other failure counts as
/// live. The file is never opened through a link.
pub(super) fn singleton_live(root: &Path) -> bool {
    match std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(root.join("lockfile"))
    {
        Ok(_) => false,
        Err(error) => error.kind() != io::ErrorKind::NotFound,
    }
}

/// Hand a validated http(s) link to the person's default browser
/// (`ShellExecuteW`'s `open`), on the calling thread. Returns whether the
/// shell accepted it. (Tests record links instead of opening them.)
#[cfg(not(test))]
pub(super) fn shell_open(url: &str) -> bool {
    let operation = wide(OsStr::new("open"));
    let file = wide(OsStr::new(url));
    // SAFETY: plain calls with NUL-terminated strings. The shell may use
    // COM, so this thread joins a single-threaded apartment around the call
    // and leaves it if joining succeeded.
    unsafe {
        let joined = CoInitializeEx(
            std::ptr::null_mut(),
            COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE,
        ) >= 0;
        let result = ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
        if joined {
            CoUninitialize();
        }
        result as isize > 32
    }
}

/// The App's end of a browser's control pipe. Windows has no portable
/// non-blocking write on it, so a dedicated thread performs each blocking
/// write and the caller waits for it until the deadline, as the Linux
/// non-blocking writer does: a write the browser does not read in time
/// times out, and the next one first waits for it. Closing ends the thread
/// (and with it the pipe) after any write in progress; that write is
/// cancelled, so a browser that stopped reading still sees the pipe close.
pub(super) struct Input {
    requests: Option<mpsc::Sender<Vec<u8>>>,
    done: mpsc::Receiver<io::Result<()>>,
    /// A write was handed over and its result has not been received.
    pending: bool,
    /// Keeps the pipe handle valid for cancelling while the thread writes.
    pipe: Arc<ChildStdin>,
    thread: Option<JoinHandle<()>>,
}

impl Input {
    pub(super) fn new(stdin: ChildStdin) -> io::Result<Self> {
        let pipe = Arc::new(stdin);
        let writer = pipe.clone();
        let (requests, received) = mpsc::channel::<Vec<u8>>();
        let (report, done) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("browser-process-writer".into())
            .spawn(move || {
                for bytes in received {
                    let result = (&*writer)
                        .write_all(&bytes)
                        .and_then(|()| (&*writer).flush());
                    let failed = result.is_err();
                    if report.send(result).is_err() || failed {
                        break;
                    }
                }
            })?;
        Ok(Self {
            requests: Some(requests),
            done,
            pending: false,
            pipe,
            thread: Some(thread),
        })
    }

    pub(super) fn write_until(&mut self, bytes: &[u8], deadline: Instant) -> io::Result<()> {
        if self.pending {
            self.settle(deadline)?;
        }
        if Instant::now() >= deadline {
            return Err(timed_out());
        }
        let sent = self
            .requests
            .as_ref()
            .is_some_and(|requests| requests.send(bytes.to_vec()).is_ok());
        if !sent {
            return Err(closed());
        }
        self.pending = true;
        self.settle(deadline)
    }

    fn settle(&mut self, deadline: Instant) -> io::Result<()> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match self.done.recv_timeout(remaining) {
            Ok(result) => {
                self.pending = false;
                result
            }
            Err(mpsc::RecvTimeoutError::Timeout) => Err(timed_out()),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.pending = false;
                Err(closed())
            }
        }
    }
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "Browser pipe write timed out")
}
fn closed() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "Browser pipe closed")
}

impl Drop for Input {
    fn drop(&mut self) {
        // The thread ends once its channel closes and any write returns.
        self.requests.take();
        if self.pending {
            // SAFETY: `pipe` keeps the handle open for this call; cancelling
            // only ends the write in progress on it (if any) with an error.
            unsafe {
                CancelIoEx(self.pipe.as_raw_handle().cast(), std::ptr::null_mut());
            }
        } else if let Some(thread) = self.thread.take() {
            // Idle: it ends at once, closing the pipe before this returns.
            let _ = thread.join();
        }
    }
}
