//! Windows (W7): an orderly end for the browser process before it returns
//! to CEF's bootstrap.
//!
//! The bootstrap owns Chromium's sandbox broker. When the browser's
//! `RunWinMain` returns, the bootstrap returns from `wWinMain` and its C
//! runtime runs the broker's `_onexit` handler (`SingletonBase::OnExit`,
//! sandbox/win/src/win_utils.h), whose destructor
//! (`BrokerServicesBase::~BrokerServicesBase`, broker_services.cc) posts
//! `THREAD_CTRL_QUIT` to the broker's job-event thread and gives that thread
//! 5000 ms to end, or terminates the process with
//! `SBOX_FATAL_BROKER_SHUTDOWN_HUNG` (7014). Ending, the thread destroys
//! every job tracker still listed: one per sandboxed target whose job has
//! not yet reported its last process gone (`JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO`
//! removes it earlier). Each such tracker terminates its job, then its
//! target (`TargetProcess::~TargetProcess`) waits up to 50 ms for the
//! process, terminates it and unregisters its IPC waits, which blocks on any
//! callback in flight; one after another. A heavy page (many site-isolated
//! frames, each a renderer, plus utilities) can leave dozens of targets
//! still ending when CefShutdown returns, since Chromium terminates its
//! children without waiting (`Process::Terminate(code, false)`), and their
//! teardown then outlasts the 5 s.
//!
//! So after CefShutdown, and before returning to the bootstrap, the browser
//! waits, bounded, for its own child processes (every sandboxed target is
//! one: the broker creates them) to have ended, so their trackers are gone
//! by the time the broker's thread is told to quit. It then writes one
//! evidence line to its standard error (the guard's log).
//!
//! The pure parts build everywhere and are tested with plain
//! `rustc --test` (std only, no CEF); the Win32 parts are Windows only.

use std::time::{Duration, Instant};

/// How long the browser waits for its children after CefShutdown. The App
/// gives a browser that answered `stopped` 30 s (`STOP_FINISH` in the App's
/// process adapter) to end with its guard's normal receipt; after `stopped`
/// come CefShutdown, this wait, the broker's own teardown (at most 5 s, then
/// 7014) and the guard's drain (0.5 s natural, then at most 2 s forced):
/// 10 + 5 + 2.5 leaves 12.5 s for CefShutdown and the rest. Usually the
/// children are gone well before it (Chromium has already terminated
/// them); the W3/W4 observer gives the next profile's dialog 20 s after a
/// partial quit (`END_WAIT`), which a full wait fits only with a short
/// CefShutdown.
pub const CHILD_WAIT: Duration = Duration::from_secs(10);

/// The evidence line's prefix on the browser's standard error.
pub const EVIDENCE_PREFIX: &str = "takosumi-cef-browser-process: shutdown ";

/// What the wait found.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ChildWait {
    /// Child processes still running after CefShutdown (each counted once,
    /// also one seen only on a later look).
    pub children: usize,
    /// Of those, still running (or not openable and still listed) when the
    /// wait ended.
    pub remaining: usize,
    /// Children that could not be opened to wait on (not counted when their
    /// id no longer named a process).
    pub unopened: usize,
    /// How long the wait took.
    pub waited: Duration,
    /// Why the wait could not look at all, if so.
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

/// Milliseconds from `now` until `deadline` for a Win32 wait: 0 once it has
/// passed, and never `INFINITE` (`u32::MAX`).
pub fn millis_left(now: Instant, deadline: Instant) -> u32 {
    let left = deadline.saturating_duration_since(now).as_millis();
    u32::try_from(left)
        .unwrap_or(u32::MAX - 1)
        .min(u32::MAX - 1)
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

/// The evidence line: `EVIDENCE_PREFIX` and one JSON object with the
/// browser's pid (so a log shared by several launches attributes it), the
/// wait's counts and duration, its bound and the exit code `RunWinMain`
/// returns to the bootstrap.
pub fn evidence_line(pid: u32, wait: &ChildWait, bound: Duration, exit_code: i32) -> String {
    let error = wait
        .error
        .as_deref()
        .map(|text| format!(",\"error\":{}", json_string(text)))
        .unwrap_or_default();
    format!(
        "{EVIDENCE_PREFIX}{{\"pid\":{pid},\"children\":{},\"remaining\":{},\"unopened\":{},\
         \"waitedMs\":{},\"boundMs\":{},\"exitCode\":{exit_code}{error}}}",
        wait.children,
        wait.remaining,
        wait.unopened,
        wait.waited.as_millis(),
        bound.as_millis(),
    )
}

/// After CefShutdown: wait for this process's children (bounded by
/// `CHILD_WAIT`) and write the evidence line with `exit_code`.
#[cfg(windows)]
pub fn finish(exit_code: i32) {
    let wait = wait_for_children(CHILD_WAIT);
    // SAFETY: no arguments; always succeeds.
    let pid = unsafe { win32::GetCurrentProcessId() };
    eprintln!("{}", evidence_line(pid, &wait, CHILD_WAIT, exit_code));
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

#[cfg(windows)]
impl Owned {
    /// Whether it has ended, waiting at most `milliseconds`.
    fn ended(&self, milliseconds: u32) -> bool {
        // SAFETY: a valid handle with SYNCHRONIZE access.
        unsafe { win32::WaitForSingleObject(self.0, milliseconds) == win32::WAIT_OBJECT_0 }
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

/// Wait until every child process of this one has ended, or `bound` has
/// passed. A child is a process whose parent id is ours and that was created
/// after us (an older process whose parent id merely names a reused id is
/// not). Children that cannot be opened are looked for again every 20 ms.
#[cfg(windows)]
pub fn wait_for_children(bound: Duration) -> ChildWait {
    use std::collections::BTreeSet;
    let started = Instant::now();
    let deadline = started + bound;
    let mut wait = ChildWait::default();
    // SAFETY: no arguments; the pseudo handle needs no closing.
    let (me, born) = unsafe {
        (
            win32::GetCurrentProcessId(),
            created(win32::GetCurrentProcess()),
        )
    };
    let Some(born) = born else {
        wait.error = Some(format!(
            "GetProcessTimes: {}",
            std::io::Error::last_os_error()
        ));
        wait.waited = started.elapsed();
        return wait;
    };
    let mut counted = BTreeSet::new();
    loop {
        let entries = match snapshot() {
            Ok(entries) => entries,
            Err(error) => {
                wait.error = Some(format!("CreateToolhelp32Snapshot: {error}"));
                break;
            }
        };
        let mut live = Vec::new();
        let mut unopened = 0;
        for pid in children_of(&entries, me) {
            let access = win32::SYNCHRONIZE | win32::PROCESS_QUERY_LIMITED_INFORMATION;
            // SAFETY: a plain OpenProcess; the handle is owned by `Owned`.
            let handle = unsafe { win32::OpenProcess(access, 0, pid) };
            if handle.is_null() {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(win32::ERROR_INVALID_PARAMETER) {
                    counted.insert(pid);
                    unopened += 1;
                }
                continue;
            }
            let process = Owned(handle);
            if created(process.0).is_some_and(|time| time < born) || process.ended(0) {
                continue;
            }
            counted.insert(pid);
            live.push(process);
        }
        wait.children = counted.len();
        wait.unopened = unopened;
        if live.is_empty() && unopened == 0 {
            wait.remaining = 0;
            break;
        }
        for process in &live {
            process.ended(millis_left(Instant::now(), deadline));
        }
        let still = live.iter().filter(|process| !process.ended(0)).count();
        if Instant::now() >= deadline {
            wait.remaining = still + unopened;
            break;
        }
        // Children seen ended: look again at once (one started meanwhile?).
        // Otherwise (one could not be opened, or a wait failed) look again
        // shortly, never past the deadline.
        if still > 0 || unopened > 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            std::thread::sleep(Duration::from_millis(20).min(left));
        }
    }
    wait.waited = started.elapsed();
    wait
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
    fn the_time_left_never_waits_forever() {
        let now = Instant::now();
        assert_eq!(millis_left(now, now), 0);
        assert_eq!(millis_left(now + Duration::from_secs(1), now), 0);
        assert_eq!(millis_left(now, now + Duration::from_millis(1500)), 1500);
        assert_eq!(
            millis_left(now, now + Duration::from_secs(60 * 60 * 24 * 60)),
            u32::MAX - 1
        );
    }

    #[test]
    fn the_evidence_line_is_one_json_object_after_its_prefix() {
        let wait = ChildWait {
            children: 31,
            remaining: 2,
            unopened: 1,
            waited: Duration::from_millis(10_000),
            error: None,
        };
        assert_eq!(
            evidence_line(4242, &wait, CHILD_WAIT, 0),
            "takosumi-cef-browser-process: shutdown {\"pid\":4242,\"children\":31,\"remaining\":2,\
             \"unopened\":1,\"waitedMs\":10000,\"boundMs\":10000,\"exitCode\":0}"
        );
        let failed = ChildWait {
            error: Some("CreateToolhelp32Snapshot: \"denied\"\n\\".into()),
            ..ChildWait::default()
        };
        assert_eq!(
            evidence_line(7, &failed, Duration::from_millis(250), 1),
            "takosumi-cef-browser-process: shutdown {\"pid\":7,\"children\":0,\"remaining\":0,\
             \"unopened\":0,\"waitedMs\":0,\"boundMs\":250,\"exitCode\":1,\
             \"error\":\"CreateToolhelp32Snapshot: \\\"denied\\\"\\n\\\\\"}"
        );
        assert_eq!(json_string("\u{1}é"), "\"\\u0001é\"");
    }

    /// The bound fits the App's 30 s for a browser that answered `stopped`,
    /// with the broker's 5 s and the guard's 2.5 s drain after it.
    #[test]
    fn the_wait_fits_the_apps_stop_budget() {
        let after_stopped = CHILD_WAIT + Duration::from_secs(5) + Duration::from_millis(2_500);
        assert!(after_stopped <= Duration::from_secs(30) - Duration::from_secs(10));
    }

    /// Windows: this test binary's own children (it re-executes itself as
    /// `sleeper`); one test at a time, since all look at the same process.
    #[cfg(windows)]
    mod windows {
        use super::super::*;
        use std::process::{Child, Command};
        use std::sync::Mutex;

        static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
        const SLEEP: &str = "TAKOSUMI_CHILD_WAIT_SLEEP_MS";

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
        /// relative to them. A zero bound only looks.
        fn baseline() -> usize {
            let wait = wait_for_children(Duration::ZERO);
            assert_eq!(wait.error, None, "{wait:?}");
            wait.remaining
        }

        #[test]
        fn children_that_end_are_waited_for() {
            let _one = ONE_AT_A_TIME
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let before = baseline();
            let mut children = sleepers(3, 400);
            // With a child that runs on, the wait runs to its bound.
            let bound = Duration::from_secs(if before == 0 { 10 } else { 3 });
            let wait = wait_for_children(bound);
            assert_eq!(
                (wait.children, wait.remaining, wait.error.clone()),
                (before + 3, before, None),
                "{wait:?}"
            );
            if before == 0 {
                assert!(wait.waited < bound, "{wait:?}");
            }
            for child in &mut children {
                assert!(
                    child.try_wait().unwrap().is_some(),
                    "a child still runs after the wait"
                );
            }
        }

        #[test]
        fn the_wait_stops_at_its_bound() {
            let _one = ONE_AT_A_TIME
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let before = baseline();
            let mut children = sleepers(2, 30_000);
            let started = Instant::now();
            let wait = wait_for_children(Duration::from_millis(300));
            let elapsed = started.elapsed();
            for child in &mut children {
                let _ = child.kill();
                let _ = child.wait();
            }
            assert_eq!(
                (wait.children, wait.remaining, wait.error.clone()),
                (before + 2, before + 2, None),
                "{wait:?}"
            );
            assert!(wait.waited >= Duration::from_millis(300), "{wait:?}");
            assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
        }

        #[test]
        fn no_children_is_no_wait() {
            let _one = ONE_AT_A_TIME
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let before = baseline();
            let bound = Duration::from_secs(if before == 0 { 10 } else { 1 });
            let wait = wait_for_children(bound);
            assert_eq!(
                (wait.children, wait.remaining, wait.error.clone()),
                (before, before, None),
                "{wait:?}"
            );
            if before == 0 {
                assert!(wait.waited < Duration::from_secs(2), "{wait:?}");
            }
        }
    }
}
