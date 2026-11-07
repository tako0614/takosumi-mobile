//! Windows (W7): how the browser process ends after CefShutdown.
//!
//! The sandboxed browser runs inside CEF's bootstrap, which owns Chromium's
//! sandbox broker. Returning to the bootstrap runs the broker's teardown:
//! the bootstrap's C runtime calls its `_onexit` handler
//! (`SingletonBase::OnExit`, sandbox/win/src/win_utils.h) once `wWinMain`
//! returns, and `BrokerServicesBase::~BrokerServicesBase`
//! (broker_services.cc) posts `THREAD_CTRL_QUIT` to the broker's job-event
//! thread and gives it 5000 ms to end, or terminates the process with
//! `SBOX_FATAL_BROKER_SHUTDOWN_HUNG` (7014). Ending, that thread tears down
//! every sandboxed target still listed, one after another (terminate its
//! job, wait up to 50 ms for it, unregister its IPC waits). CefShutdown
//! terminates its children without waiting for them, so a heavy page leaves
//! dozens still ending: their teardown outlasted 5 s (7014), and waiting for
//! them first took 7.5 s, too long for the App's sequential quit.
//!
//! So once CefShutdown has returned, the sandboxed browser ends itself with
//! `ExitProcess` and the run's exit code, as the browser process did before
//! W7 round 2 (`std::process::exit`), and never returns to the bootstrap.
//! (CEF helper processes, which have no broker, return to the bootstrap;
//! before round 2 they too ended with `std::process::exit`.) `ExitProcess`
//! skips the executable's `_onexit` handlers (the broker's teardown) and
//! closes the process's handles, among them every target's job handle; each
//! job was created with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` (sandbox/win/src/
//! job.cc: every job level includes it), so the kernel ends the targets
//! still running. CefShutdown has already flushed the profile, so nothing
//! that writes data is skipped. Before ending, the browser counts its child
//! processes still running (no wait) and writes one evidence line to its
//! standard error (the guard's log), with how long the close and CefShutdown
//! took (`mark` records the moments). The unsandboxed candidate (an
//! executable, no bootstrap and no broker) writes the same line and returns
//! its code as before.
//!
//! The pure parts build everywhere and are tested with plain
//! `rustc --test` (std only, no CEF); the Win32 parts are Windows only.

use std::sync::{Mutex, PoisonError};
use std::time::Instant;

/// The evidence line's prefix on the browser's standard error.
pub const EVIDENCE_PREFIX: &str = "takosumi-cef-browser-process: shutdown ";

/// A moment of the browser's end that the evidence line measures from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// The App's Close or Shutdown request (or the end of its input)
    /// reached the UI thread; the latest one counts.
    CloseRequested,
    /// A browser window's `on_before_close`; the latest (the last window's)
    /// counts.
    BrowserClosed,
    /// The message loop was asked to quit (CefShutdown follows).
    Quit,
}

/// The moments recorded so far.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Marks {
    pub close_requested: Option<Instant>,
    pub browser_closed: Option<Instant>,
    pub quit: Option<Instant>,
}

static MARKS: Mutex<Marks> = Mutex::new(Marks {
    close_requested: None,
    browser_closed: None,
    quit: None,
});

/// Record `which` now (the latest of each kind is kept).
pub fn mark(which: Mark) {
    let now = Instant::now();
    let mut marks = MARKS.lock().unwrap_or_else(PoisonError::into_inner);
    match which {
        Mark::CloseRequested => marks.close_requested = Some(now),
        Mark::BrowserClosed => marks.browser_closed = Some(now),
        Mark::Quit => marks.quit = Some(now),
    }
}

/// The moments recorded so far.
pub fn marks() -> Marks {
    *MARKS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How long the end took, in milliseconds (None when a moment is missing or
/// out of order): `close` from the close request to the last window's
/// `on_before_close`, `shutdown` from the quit to CefShutdown's return
/// (`ended`), `total` from the close request to `ended`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Durations {
    pub close: Option<u128>,
    pub shutdown: Option<u128>,
    pub total: Option<u128>,
}

pub fn durations(marks: &Marks, ended: Instant) -> Durations {
    let between = |from: Option<Instant>, to: Option<Instant>| {
        to?.checked_duration_since(from?)
            .map(|duration| duration.as_millis())
    };
    Durations {
        close: between(marks.close_requested, marks.browser_closed),
        shutdown: between(marks.quit, Some(ended)),
        total: between(marks.close_requested, Some(ended)),
    }
}

/// How the browser process ends after CefShutdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// The sandboxed browser: `ExitProcess` (no return to the bootstrap).
    ExitProcess,
    /// The unsandboxed candidate: it returns its code to `main`.
    Return,
}

impl End {
    pub fn name(self) -> &'static str {
        match self {
            End::ExitProcess => "exit-process",
            End::Return => "return",
        }
    }
}

/// The child processes still running when the browser ends.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Census {
    /// Children seen running (each opened and not yet ended).
    pub children: usize,
    /// Children that could not be opened to tell (not counted when their id
    /// no longer named a process).
    pub unopened: usize,
    /// Why the browser could not look at all, if so.
    pub error: Option<String>,
}

/// The direct children of `parent` among a process snapshot's
/// (pid, parent pid) entries: sorted, once each, never `parent` itself or
/// the idle process (0).
pub fn children_of(entries: &[(u32, u32)], parent: u32) -> Vec<u32> {
    let mut found: Vec<u32> = entries
        .iter()
        .filter(|&&(pid, of)| of == parent && pid != parent && pid != 0)
        .map(|&(pid, _)| pid)
        .collect();
    found.sort_unstable();
    found.dedup();
    found
}

/// `text` as a JSON string.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A duration for the evidence line: its milliseconds, or `null`.
fn millis(value: Option<u128>) -> String {
    value.map_or_else(|| "null".into(), |ms| ms.to_string())
}

/// The evidence line: `EVIDENCE_PREFIX` and one JSON object with the
/// browser's pid (so a log shared by several launches attributes it), the
/// children still running, how it ends, its exit code and how long the
/// close (`closeMs`), CefShutdown (`shutdownMs`) and the whole end
/// (`totalMs`) took.
pub fn evidence_line(
    pid: u32,
    census: &Census,
    end: End,
    exit_code: i32,
    took: &Durations,
) -> String {
    let error = census
        .error
        .as_deref()
        .map(|text| format!(",\"error\":{}", json_string(text)))
        .unwrap_or_default();
    format!(
        "{EVIDENCE_PREFIX}{{\"pid\":{pid},\"children\":{},\"unopened\":{},\"end\":\"{}\",\
         \"exitCode\":{exit_code},\"closeMs\":{},\"shutdownMs\":{},\"totalMs\":{}{error}}}",
        census.children,
        census.unopened,
        end.name(),
        millis(took.close),
        millis(took.shutdown),
        millis(took.total),
    )
}

/// After CefShutdown: count this process's running children, write the
/// evidence line and, for the sandboxed browser (`exit`), end the process
/// with `ExitProcess(exit_code)`; otherwise return. A failed log write is
/// ignored (it must not turn a clean end into a panic).
#[cfg(windows)]
pub fn finish(exit_code: i32, exit: bool) {
    use std::io::Write;
    let took = durations(&marks(), Instant::now());
    let end = if exit { End::ExitProcess } else { End::Return };
    // SAFETY: no arguments; always succeeds.
    let pid = unsafe { win32::GetCurrentProcessId() };
    let line = evidence_line(pid, &census(), end, exit_code, &took);
    let _ = writeln!(std::io::stderr(), "{line}");
    if exit {
        exit_process(exit_code);
    }
}

/// End this process now with `exit_code`, after flushing standard output
/// and error: `ExitProcess`, so CEF's bootstrap never runs its teardown of
/// the sandbox broker; the kernel closes the process's handles, among them
/// every target's kill-on-close job.
#[cfg(windows)]
pub fn exit_process(exit_code: i32) -> ! {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    // SAFETY: ends the process.
    unsafe { win32::ExitProcess(exit_code as u32) }
}

#[cfg(windows)]
mod win32 {
    use std::ffi::c_void;
    pub type Handle = *mut c_void;
    pub const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
    pub const TH32CS_SNAPPROCESS: u32 = 0x2;
    pub const SYNCHRONIZE: u32 = 0x0010_0000;
    pub const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    pub const WAIT_OBJECT_0: u32 = 0;
    /// `OpenProcess` of an id no process has any more.
    pub const ERROR_INVALID_PARAMETER: i32 = 87;

    /// PROCESSENTRY32W.
    #[repr(C)]
    pub struct ProcessEntry {
        pub size: u32,
        pub usage: u32,
        pub process_id: u32,
        pub default_heap_id: usize,
        pub module_id: u32,
        pub threads: u32,
        pub parent_process_id: u32,
        pub priority_base: i32,
        pub flags: u32,
        pub exe_file: [u16; 260],
    }
    #[cfg(target_pointer_width = "64")]
    const _: () = assert!(std::mem::size_of::<ProcessEntry>() == 568);

    #[link(name = "kernel32")]
    extern "system" {
        pub fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> Handle;
        pub fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry) -> i32;
        pub fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry) -> i32;
        pub fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> Handle;
        pub fn GetProcessTimes(
            process: Handle,
            creation: *mut u64,
            exit: *mut u64,
            kernel: *mut u64,
            user: *mut u64,
        ) -> i32;
        pub fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
        pub fn CloseHandle(handle: Handle) -> i32;
        pub fn GetCurrentProcess() -> Handle;
        pub fn GetCurrentProcessId() -> u32;
        pub fn ExitProcess(exit_code: u32) -> !;
    }
}

/// An owned kernel handle (a process, or a snapshot), closed on drop.
#[cfg(windows)]
struct Owned(win32::Handle);

#[cfg(windows)]
impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: an owned handle, closed once.
        unsafe { win32::CloseHandle(self.0) };
    }
}

/// A process's creation time (FILETIME as one value), if readable.
#[cfg(windows)]
fn created(process: win32::Handle) -> Option<u64> {
    let (mut creation, mut exit, mut kernel, mut user) = (0u64, 0u64, 0u64, 0u64);
    // SAFETY: valid out pointers; FILETIME is two u32s, laid out as one
    // little-endian u64 and at least as aligned.
    let read = unsafe {
        win32::GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user)
    };
    (read != 0).then_some(creation)
}

/// Every process's (pid, parent pid), from a Toolhelp snapshot.
#[cfg(windows)]
fn snapshot() -> std::io::Result<Vec<(u32, u32)>> {
    // SAFETY: plain Toolhelp calls on a snapshot handle closed below.
    unsafe {
        let snapshot = win32::CreateToolhelp32Snapshot(win32::TH32CS_SNAPPROCESS, 0);
        if snapshot == win32::INVALID_HANDLE_VALUE || snapshot.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let snapshot = Owned(snapshot);
        let mut found = Vec::new();
        let mut entry: win32::ProcessEntry = std::mem::zeroed();
        entry.size = std::mem::size_of::<win32::ProcessEntry>() as u32;
        let mut more = win32::Process32FirstW(snapshot.0, &mut entry);
        while more != 0 {
            found.push((entry.process_id, entry.parent_process_id));
            entry.size = std::mem::size_of::<win32::ProcessEntry>() as u32;
            more = win32::Process32NextW(snapshot.0, &mut entry);
        }
        Ok(found)
    }
}

/// This process's children still running, now (no wait). A child is a
/// process whose parent id is ours and that was created after us (an older
/// process whose parent id merely names a reused id is not).
#[cfg(windows)]
pub fn census() -> Census {
    let mut census = Census::default();
    // SAFETY: no arguments; the pseudo handle needs no closing.
    let (me, born) = unsafe {
        (
            win32::GetCurrentProcessId(),
            created(win32::GetCurrentProcess()),
        )
    };
    let Some(born) = born else {
        census.error = Some(format!(
            "GetProcessTimes: {}",
            std::io::Error::last_os_error()
        ));
        return census;
    };
    let entries = match snapshot() {
        Ok(entries) => entries,
        Err(error) => {
            census.error = Some(format!("CreateToolhelp32Snapshot: {error}"));
            return census;
        }
    };
    for pid in children_of(&entries, me) {
        let access = win32::SYNCHRONIZE | win32::PROCESS_QUERY_LIMITED_INFORMATION;
        // SAFETY: a plain OpenProcess; the handle is owned by `Owned`.
        let handle = unsafe { win32::OpenProcess(access, 0, pid) };
        if handle.is_null() {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(win32::ERROR_INVALID_PARAMETER) {
                census.unopened += 1;
            }
            continue;
        }
        let process = Owned(handle);
        // SAFETY: a valid handle with SYNCHRONIZE access; no wait.
        let ended = unsafe { win32::WaitForSingleObject(process.0, 0) == win32::WAIT_OBJECT_0 };
        if !ended && created(process.0).is_none_or(|time| time >= born) {
            census.children += 1;
        }
    }
    census
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn children_are_the_direct_children_once_each() {
        let entries = [
            (0, 0),
            (4, 0),
            (10, 4),
            (21, 10),
            (20, 10),
            (30, 20),
            (20, 10),
            (10, 10),
        ];
        assert_eq!(children_of(&entries, 10), vec![20, 21]);
        assert_eq!(children_of(&entries, 30), Vec::<u32>::new());
        assert_eq!(children_of(&entries, 0), vec![4]);
    }

    #[test]
    fn the_evidence_line_is_one_json_object_after_its_prefix() {
        let census = Census {
            children: 65,
            unopened: 1,
            error: None,
        };
        let took = Durations {
            close: Some(1250),
            shutdown: Some(2300),
            total: Some(4100),
        };
        assert_eq!(
            evidence_line(4242, &census, End::ExitProcess, 0, &took),
            "takosumi-cef-browser-process: shutdown {\"pid\":4242,\"children\":65,\"unopened\":1,\
             \"end\":\"exit-process\",\"exitCode\":0,\"closeMs\":1250,\"shutdownMs\":2300,\
             \"totalMs\":4100}"
        );
        let failed = Census {
            error: Some("CreateToolhelp32Snapshot: \"denied\"\n\\".into()),
            ..Census::default()
        };
        assert_eq!(
            evidence_line(7, &failed, End::Return, 1, &Durations::default()),
            "takosumi-cef-browser-process: shutdown {\"pid\":7,\"children\":0,\"unopened\":0,\
             \"end\":\"return\",\"exitCode\":1,\"closeMs\":null,\"shutdownMs\":null,\"totalMs\":null,\
             \"error\":\"CreateToolhelp32Snapshot: \\\"denied\\\"\\n\\\\\"}"
        );
        assert_eq!(json_string("\u{1}é"), "\"\\u0001é\"");
    }

    #[test]
    fn the_durations_split_the_close_from_cef_shutdown() {
        use std::time::Duration;
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let recorded = Marks {
            close_requested: Some(at(100)),
            browser_closed: Some(at(1350)),
            quit: Some(at(1400)),
        };
        assert_eq!(
            durations(&recorded, at(3700)),
            Durations {
                close: Some(1250),
                shutdown: Some(2300),
                total: Some(3600),
            }
        );
        // A missing moment, or one out of order, gives no duration.
        let partial = Marks {
            close_requested: None,
            browser_closed: Some(at(50)),
            quit: Some(at(60)),
        };
        assert_eq!(
            durations(&partial, at(80)),
            Durations {
                close: None,
                shutdown: Some(20),
                total: None,
            }
        );
        let reversed = Marks {
            close_requested: Some(at(500)),
            browser_closed: Some(at(400)),
            quit: None,
        };
        assert_eq!(durations(&reversed, at(600)).close, None);
        // The latest mark of a kind is kept.
        mark(Mark::Quit);
        let first = marks().quit.unwrap();
        std::thread::sleep(Duration::from_millis(5));
        mark(Mark::Quit);
        assert!(marks().quit.unwrap() > first);
    }

    /// Windows: this test binary's own children (it re-executes itself as
    /// `sleeper`); one test at a time, since all look at the same process.
    #[cfg(windows)]
    mod windows {
        use super::super::*;
        use std::process::{Child, Command};
        use std::sync::Mutex;
        use std::time::Duration;

        static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
        const SLEEP: &str = "TAKOSUMI_CHILD_CENSUS_SLEEP_MS";

        /// Run as a child only: sleeps as long as its parent asked.
        #[test]
        fn sleeper() {
            if let Some(ms) = std::env::var(SLEEP)
                .ok()
                .and_then(|value| value.parse().ok())
            {
                std::thread::sleep(Duration::from_millis(ms));
            }
        }

        fn sleepers(count: usize, ms: u64) -> Vec<Child> {
            // libtest names a test by its path without the crate's name.
            let module = module_path!().split_once("::").map_or("", |(_, rest)| rest);
            let name = format!("{module}::sleeper");
            (0..count)
                .map(|_| {
                    Command::new(std::env::current_exe().unwrap())
                        .args(["--exact", &name, "--test-threads", "1", "--quiet"])
                        .env(SLEEP, ms.to_string())
                        .stdout(std::process::Stdio::null())
                        .spawn()
                        .unwrap()
                })
                .collect()
        }

        /// Children this test binary already has that run on (a console
        /// host, when it was started without a console): the tests count
        /// relative to them.
        fn baseline() -> usize {
            let census = census();
            assert_eq!(census.error, None, "{census:?}");
            census.children
        }

        #[test]
        fn running_children_are_counted_without_waiting() {
            let _one = ONE_AT_A_TIME
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let before = baseline();
            let mut children = sleepers(3, 30_000);
            let started = std::time::Instant::now();
            let census = census();
            let elapsed = started.elapsed();
            for child in &mut children {
                let _ = child.kill();
                let _ = child.wait();
            }
            assert_eq!(
                (census.children, census.unopened, census.error.clone()),
                (before + 3, 0, None),
                "{census:?}"
            );
            assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
        }

        #[test]
        fn ended_children_are_not_counted() {
            let _one = ONE_AT_A_TIME
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let before = baseline();
            let mut children = sleepers(2, 10);
            for child in &mut children {
                child.wait().unwrap();
            }
            // Still held (not reaped by a dropped handle): ended, not counted.
            let census = census();
            assert_eq!(
                (census.children, census.error.clone()),
                (before, None),
                "{census:?}"
            );
            drop(children);
        }
    }
}
