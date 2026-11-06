//! Self-tests for the Windows `takosumi-browser-guard` with stand-in browsers.
//!
//! As on Linux the bin uses `harness = false`: this runner re-executes the
//! self-test binary with `AS_GUARD` set, so each case drives the same
//! `prepare`/`supervise` code a release build runs, started inside a fresh
//! kill-on-close Job Object as the App starts it. The self-test binary is
//! also the stand-in browser (`--standin <mode>`), so nothing but the guard
//! itself is needed. `cargo test --bin takosumi-browser-guard [-- <filter>]`.

use super::*;
use serde_json::Value;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::mpsc;
use tauri_plugin_app_screen::windows_job::DETACHED_PROCESS;

pub(crate) const AS_GUARD: &str = "TAKOSUMI_BROWSER_GUARD_SELF_TEST_AS_GUARD";
const STANDIN: &str = "--standin";

/// What this self-test binary is: a stand-in browser (its first argument is
/// `--standin`), the guard under test (`AS_GUARD` is set; `None`), or the
/// test runner.
pub(crate) fn dispatch() -> Option<ExitCode> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some(STANDIN) {
        return Some(standin(&args[2..]));
    }
    if std::env::var_os(AS_GUARD).is_some() {
        return None;
    }
    Some(run())
}

/// The stand-in browser's modes; it reports on its stdout (the guard's).
fn standin(args: &[String]) -> ExitCode {
    let say = |line: String| {
        let mut out = io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    };
    match args.first().map(String::as_str) {
        Some("exit") => ExitCode::from(args[1].parse::<u8>().unwrap()),
        // Echo one control-pipe line; 7 if the stop event's name leaked.
        Some("echo") => {
            let mut line = String::new();
            io::stdin().lock().read_line(&mut line).unwrap();
            say(line.trim_end().to_string());
            ExitCode::from(if std::env::var_os(STOP_EVENT_ENV).is_some() {
                7
            } else {
                0
            })
        }
        // Leave a descendant behind (in the job, not holding the pipes): it
        // is deliberately never waited for.
        Some("linger") => {
            #[allow(clippy::zombie_processes)]
            let helper = Command::new(std::env::current_exe().unwrap())
                .args([STANDIN, "sleep"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            say(format!("linger {}", helper.id()));
            ExitCode::SUCCESS
        }
        Some("sleep") => {
            say(format!("ready {}", std::process::id()));
            std::thread::sleep(Duration::from_secs(60));
            ExitCode::SUCCESS
        }
        Some("marker") => {
            std::fs::write(&args[1], b"ran").unwrap();
            ExitCode::SUCCESS
        }
        other => panic!("unknown stand-in mode {other:?}"),
    }
}

type Test = (&'static str, fn());
macro_rules! tests {
    ($($name:ident),* $(,)?) => { &[$((stringify!($name), $name as fn())),*] };
}
const TESTS: &[Test] = tests![
    normal_exit_writes_a_complete_normal_receipt,
    a_non_zero_browser_exit_is_reported_and_fails,
    the_browser_keeps_the_pipes_but_not_the_stop_event,
    a_lingering_descendant_is_terminated_with_the_job,
    a_job_that_empties_as_the_natural_budget_ends_is_not_forced,
    a_stop_request_drains_a_running_browser,
    ending_the_apps_job_ends_the_guard_and_the_browser,
    a_pending_stop_starts_no_browser,
    refusals_create_no_receipt_and_start_nothing,
    a_browser_that_cannot_start_leaves_an_incomplete_receipt,
];

/// Runs the selected tests one at a time. Arguments without a leading `-`
/// filter test names by substring. Selecting nothing is a failure.
fn run() -> ExitCode {
    let filters: Vec<String> = std::env::args()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .collect();
    let selected: Vec<&Test> = TESTS
        .iter()
        .filter(|(name, _)| filters.is_empty() || filters.iter().any(|f| name.contains(f.as_str())))
        .collect();
    println!("\nrunning {} tests", selected.len());
    let mut failed = Vec::new();
    for (name, test) in &selected {
        let started = Instant::now();
        let outcome = std::panic::catch_unwind(*test);
        let seconds = started.elapsed().as_secs_f32();
        if outcome.is_ok() {
            println!("test {name} ... ok ({seconds:.2}s)");
        } else {
            println!("test {name} ... FAILED ({seconds:.2}s)");
            failed.push(*name);
        }
    }
    let passed = selected.len() - failed.len();
    if failed.is_empty() && !selected.is_empty() {
        println!("\ntest result: ok. {passed} passed; 0 failed\n");
        ExitCode::SUCCESS
    } else {
        println!("\nfailures: {failed:?}");
        println!(
            "test result: FAILED. {passed} passed; {} failed\n",
            failed.len()
        );
        ExitCode::FAILURE
    }
}

/// One case: its directory (kept), the receipt position, the App's stop
/// event and the App's job the guard runs in.
struct Case {
    dir: PathBuf,
    receipt: PathBuf,
    event_name: String,
    event: Event,
    job: Option<Job>,
}

impl Case {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let unique = format!("{name}-{}-{nanos}", std::process::id());
        let dir = std::env::temp_dir().join(format!("sumi-gw-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        let event_name = format!(r"Local\takosumi-guard-self-test-{unique}");
        Self {
            receipt: dir.join("guard.json"),
            dir,
            event: Event::create(&event_name).unwrap(),
            event_name,
            job: Some(Job::kill_on_close().unwrap()),
        }
    }

    /// The guard with a stand-in browser in `mode`.
    fn guard(&self, mode: &[&str]) -> Command {
        let exe = std::env::current_exe().unwrap();
        let mut command = Command::new(&exe);
        command
            .arg(&self.receipt)
            .arg(&exe)
            .arg(STANDIN)
            .args(mode)
            .env(AS_GUARD, "1")
            .env(STOP_EVENT_ENV, &self.event_name)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        command
    }

    /// Started inside the case's job and detached from any console, as the
    /// App starts it.
    fn start(&self, mut command: Command) -> Child {
        spawn_in(self.job.as_ref().unwrap(), &mut command, DETACHED_PROCESS).unwrap()
    }

    fn receipt(&self) -> Value {
        serde_json::from_slice(&std::fs::read(&self.receipt).unwrap()).unwrap()
    }

    /// Nothing of the case is left in the App's job (the guard and everything
    /// below it). Called after the guard's exit was observed and its receipt
    /// proved its own job empty, so a process still counted can only be one
    /// whose teardown is finishing. That takes milliseconds; 3 s allows for a
    /// loaded runner yet stays far below any stand-in's own lifetime (a
    /// sleeping stand-in runs 60 s), so no real survivor passes. A failure
    /// names every process still in the job with its image.
    fn assert_nothing_left(&self) {
        let job = self.job.as_ref().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let active = job.processes().unwrap().1;
            if active == 0 {
                return;
            }
            if Instant::now() >= deadline {
                let left: Vec<String> = job
                    .process_ids()
                    .unwrap()
                    .into_iter()
                    .map(|pid| format!("{pid} {:?}", image_of(pid)))
                    .collect();
                panic!("{active} processes are left in the App's job: {left:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// The executable image of a process, for failure messages.
fn image_of(pid: u32) -> Option<String> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStringExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn QueryFullProcessImageNameW(
            process: *mut c_void,
            flags: u32,
            name: *mut u16,
            size: *mut u32,
        ) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    let mut buffer = vec![0u16; 32_768];
    let mut size = buffer.len() as u32;
    // SAFETY: plain calls; the handle is checked, used for one query with a
    // buffer of `size` units, and closed.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let queried = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut size);
        CloseHandle(process);
        (queried != 0).then(|| {
            std::ffi::OsString::from_wide(&buffer[..size as usize])
                .to_string_lossy()
                .into_owned()
        })
    }
}

/// Lines from a guard's stdout, read on a thread so every read is bounded.
struct Lines(mpsc::Receiver<Option<String>>);

impl Lines {
    fn of(child: &mut Child) -> Self {
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let (send, lines) = mpsc::channel();
        std::thread::spawn(move || loop {
            let mut line = String::new();
            let read = reader.read_line(&mut line).unwrap_or(0);
            let _ = send.send((read > 0).then(|| line.trim_end().to_string()));
            if read == 0 {
                break;
            }
        });
        Self(lines)
    }

    /// The next line, or None at end of file; fails after `limit`.
    fn next(&self, limit: Duration) -> Option<String> {
        self.0.recv_timeout(limit).expect("no guard output in time")
    }
}

fn finish(child: &mut Child, limit: Duration) -> ExitStatus {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "the guard did not end in time");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn clean(proof: &Value) -> bool {
    proof["kernelNoChildren"] == true
        && proof["remaining"].as_array().is_some_and(Vec::is_empty)
        && proof["error"].is_null()
        && proof["cleanupErrors"].as_array().is_some_and(Vec::is_empty)
        && proof["nonFatalErrors"]
            .as_array()
            .is_some_and(Vec::is_empty)
}

fn normal_exit_writes_a_complete_normal_receipt() {
    let case = Case::new("normal");
    let mut guard = case.start(case.guard(&["exit", "0"]));
    let status = finish(&mut guard, Duration::from_secs(10));
    let proof = case.receipt();
    assert_eq!(status.code(), Some(0), "{proof}");
    assert!(clean(&proof), "{proof}");
    assert_eq!(proof["rootExit"], 0);
    assert_eq!(proof["complete"], true);
    assert_eq!(proof["forcedCleanup"], 0);
    assert!(proof["observed"].as_u64().unwrap() >= 1, "{proof}");
    let fields: BTreeSet<&str> = proof
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        fields,
        BTreeSet::from([
            "rootExit",
            "complete",
            "remaining",
            "kernelNoChildren",
            "observed",
            "forcedCleanup",
            "error",
            "cleanupErrors",
            "nonFatalErrors",
        ])
    );
    case.assert_nothing_left();
}

fn a_non_zero_browser_exit_is_reported_and_fails() {
    let case = Case::new("exit3");
    let mut guard = case.start(case.guard(&["exit", "3"]));
    let status = finish(&mut guard, Duration::from_secs(10));
    let proof = case.receipt();
    assert_eq!(status.code(), Some(1), "{proof}");
    assert!(clean(&proof), "{proof}");
    assert_eq!(proof["rootExit"], 3);
    assert_eq!(proof["complete"], true);
    assert_eq!(proof["forcedCleanup"], 0);
}

fn the_browser_keeps_the_pipes_but_not_the_stop_event() {
    let case = Case::new("pipes");
    let mut command = case.guard(&["echo"]);
    command.stdin(Stdio::piped());
    let mut guard = case.start(command);
    let lines = Lines::of(&mut guard);
    let mut input = guard.stdin.take().unwrap();
    writeln!(input, "control-pipe-line").unwrap();
    drop(input);
    assert_eq!(
        lines.next(Duration::from_secs(10)).as_deref(),
        Some("control-pipe-line")
    );
    let status = finish(&mut guard, Duration::from_secs(10));
    let proof = case.receipt();
    // 7 would mean the browser saw the stop event's name.
    assert_eq!(proof["rootExit"], 0, "{proof}");
    assert_eq!(status.code(), Some(0), "{proof}");
}

fn a_lingering_descendant_is_terminated_with_the_job() {
    let case = Case::new("linger");
    let started = Instant::now();
    let mut guard = case.start(case.guard(&["linger"]));
    let lines = Lines::of(&mut guard);
    let line = lines.next(Duration::from_secs(10)).unwrap();
    assert!(line.starts_with("linger "), "{line}");
    let status = finish(&mut guard, Duration::from_secs(10));
    let elapsed = started.elapsed();
    let proof = case.receipt();
    // As on Linux, a forced cleanup alone does not change the exit status;
    // the App refuses it through `forcedCleanup`.
    assert_eq!(status.code(), Some(0), "{proof}");
    assert!(clean(&proof), "{proof}");
    assert_eq!(proof["rootExit"], 0);
    assert_eq!(proof["complete"], true);
    assert!(proof["forcedCleanup"].as_u64().unwrap() >= 1, "{proof}");
    assert!(proof["observed"].as_u64().unwrap() >= 2, "{proof}");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    // The descendant sleeps 60 s unless the job ended it.
    case.assert_nothing_left();
}

fn a_job_that_empties_as_the_natural_budget_ends_is_not_forced() {
    // The guard under test lets its natural budget run out only once the job
    // is empty: the drain must then find nothing to terminate.
    let case = Case::new("late");
    let mut command = case.guard(&["exit", "0"]);
    command.env("TAKOSUMI_GUARD_FAULT_LATE_EMPTY", "1");
    let mut guard = case.start(command);
    let status = finish(&mut guard, Duration::from_secs(20));
    let proof = case.receipt();
    assert_eq!(status.code(), Some(0), "{proof}");
    assert!(clean(&proof), "{proof}");
    assert_eq!(proof["rootExit"], 0);
    assert_eq!(proof["complete"], true);
    assert_eq!(proof["forcedCleanup"], 0, "nothing was terminated");
    case.assert_nothing_left();
}

fn a_stop_request_drains_a_running_browser() {
    let case = Case::new("stop");
    let mut guard = case.start(case.guard(&["sleep"]));
    let lines = Lines::of(&mut guard);
    let line = lines.next(Duration::from_secs(10)).unwrap();
    assert!(line.starts_with("ready "), "{line}");
    let stopped = Instant::now();
    case.event.set().unwrap();
    let status = finish(&mut guard, Duration::from_secs(10));
    let elapsed = stopped.elapsed();
    let proof = case.receipt();
    assert_eq!(status.code(), Some(1), "{proof}");
    assert!(clean(&proof), "{proof}");
    assert_eq!(proof["complete"], true);
    assert_eq!(proof["rootExit"], FORCED_EXIT);
    assert!(proof["forcedCleanup"].as_u64().unwrap() >= 1, "{proof}");
    assert!(elapsed < Duration::from_secs(4), "{elapsed:?}");
    case.assert_nothing_left();
}

fn ending_the_apps_job_ends_the_guard_and_the_browser() {
    let mut case = Case::new("job");
    let mut guard = case.start(case.guard(&["sleep"]));
    let lines = Lines::of(&mut guard);
    let line = lines.next(Duration::from_secs(10)).unwrap();
    assert!(line.starts_with("ready "), "{line}");
    // The App ends: its job's last handle closes.
    drop(case.job.take());
    finish(&mut guard, Duration::from_secs(10));
    // End of file: no process holds the reply pipe any more, so the
    // stand-in browser ended with its guard.
    assert_eq!(lines.next(Duration::from_secs(10)), None);
    let mut written = String::new();
    File::open(&case.receipt)
        .unwrap()
        .read_to_string(&mut written)
        .unwrap();
    assert!(
        written.is_empty(),
        "a guard ended with its App writes no receipt"
    );
}

fn a_pending_stop_starts_no_browser() {
    let case = Case::new("pending");
    case.event.set().unwrap();
    let marker = case.dir.join("ran");
    let mut guard = case.start(case.guard(&["marker", marker.to_str().unwrap()]));
    let status = finish(&mut guard, Duration::from_secs(10));
    let proof = case.receipt();
    assert_eq!(status.code(), Some(1), "{proof}");
    assert_eq!(proof["error"], "StopRequestedBeforeStart");
    assert!(proof["rootExit"].is_null());
    assert_eq!(proof["complete"], false);
    assert!(!marker.exists(), "the browser never started");
}

fn refusals_create_no_receipt_and_start_nothing() {
    let refused = |case: &Case, command: Command| {
        let mut guard = case.start(command);
        assert_eq!(finish(&mut guard, Duration::from_secs(10)).code(), Some(2));
    };
    let marker_mode = |case: &Case| {
        let marker = case.dir.join("ran");
        (marker.to_str().unwrap().to_string(), marker)
    };
    // An existing receipt is never replaced.
    let case = Case::new("existing");
    std::fs::write(&case.receipt, b"earlier").unwrap();
    let (mode, marker) = marker_mode(&case);
    refused(&case, case.guard(&["marker", &mode]));
    assert_eq!(std::fs::read(&case.receipt).unwrap(), b"earlier");
    assert!(!marker.exists());
    // A relative receipt.
    let case = Case::new("relative");
    let (mode, marker) = marker_mode(&case);
    let exe = std::env::current_exe().unwrap();
    let mut relative = Command::new(&exe);
    relative
        .arg("guard.json")
        .arg(&exe)
        .args([STANDIN, "marker", &mode])
        .env(AS_GUARD, "1")
        .env(STOP_EVENT_ENV, &case.event_name)
        .current_dir(&case.dir)
        .stdout(Stdio::null());
    refused(&case, relative);
    assert!(!case.dir.join("guard.json").exists() && !marker.exists());
    // No stop event named, or one that does not exist.
    for name in [None, Some(r"Local\takosumi-guard-self-test-never-created")] {
        let case = Case::new("event");
        let (mode, marker) = marker_mode(&case);
        let mut command = case.guard(&["marker", &mode]);
        match name {
            None => command.env_remove(STOP_EVENT_ENV),
            Some(name) => command.env(STOP_EVENT_ENV, name),
        };
        refused(&case, command);
        assert!(!case.receipt.exists() && !marker.exists(), "{name:?}");
    }
    // No program.
    let case = Case::new("usage");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .arg(&case.receipt)
        .env(AS_GUARD, "1")
        .env(STOP_EVENT_ENV, &case.event_name)
        .stdout(Stdio::null());
    refused(&case, command);
    assert!(!case.receipt.exists());
}

fn a_browser_that_cannot_start_leaves_an_incomplete_receipt() {
    let case = Case::new("missing");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .arg(&case.receipt)
        .arg(case.dir.join("no-such-browser.exe"))
        .env(AS_GUARD, "1")
        .env(STOP_EVENT_ENV, &case.event_name)
        .stdout(Stdio::null());
    let mut guard = case.start(command);
    let status = finish(&mut guard, Duration::from_secs(10));
    let proof = case.receipt();
    assert_eq!(status.code(), Some(1), "{proof}");
    assert!(proof["error"].is_string(), "{proof}");
    assert!(proof["rootExit"].is_null());
    assert_eq!(proof["complete"], false);
    assert_eq!(proof["kernelNoChildren"], true);
}
