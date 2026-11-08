//! Self-tests for the macOS `takosumi-browser-guard` with stand-in browsers.
//!
//! As on Linux and Windows the bin uses `harness = false`: this runner
//! re-executes the self-test binary with `AS_GUARD` set, so each case drives
//! the same `prepare`/`supervise` code a release build runs, started as the
//! leader of its own process group as the App starts it. The self-test
//! binary is also the stand-in browser (`--standin <mode>`) and the stand-in
//! App of the parent-death case (`--app`), so nothing but the guard itself
//! is needed. `cargo test --bin takosumi-browser-guard [-- <filter>]`.

use super::*;
use serde_json::Value;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, ExitStatus, Stdio};

pub(crate) const AS_GUARD: &str = "TAKOSUMI_BROWSER_GUARD_SELF_TEST_AS_GUARD";
const STANDIN: &str = "--standin";
const APP: &str = "--app";

/// What this self-test binary is: a stand-in browser, a stand-in App, the
/// guard under test (`AS_GUARD` is set; `None`), or the test runner.
pub(crate) fn dispatch() -> Option<ExitCode> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some(STANDIN) => return Some(standin(&args[2..])),
        Some(APP) => return Some(app(&args[2..])),
        _ => {}
    }
    if std::env::var_os(AS_GUARD).is_some() {
        return None;
    }
    Some(run())
}

fn say(line: String) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn helper(mode: &[&str]) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .arg(STANDIN)
        .args(mode)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// The stand-in browser's modes; it reports on its stdout (the guard's).
fn standin(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("exit") => ExitCode::from(args[1].parse::<u8>().unwrap()),
        // Leave a mark that this browser started, then exit 0.
        Some("touch") => {
            std::fs::write(&args[1], b"started").unwrap();
            ExitCode::SUCCESS
        }
        // Echo one control-pipe line, then report its stop signals'
        // dispositions and blocked mask (the guard ignores both signals
        // itself; the browser must have them at their defaults).
        Some("echo") => {
            let mut line = String::new();
            io::stdin().lock().read_line(&mut line).unwrap();
            say(line.trim_end().to_string());
            let mut defaults = true;
            for signal in [libc::SIGTERM, libc::SIGINT] {
                // SAFETY: reading a disposition with a zeroed sigaction.
                let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
                unsafe { libc::sigaction(signal, std::ptr::null(), &mut action) };
                defaults &= action.sa_sigaction == libc::SIG_DFL;
            }
            let mut mask: libc::sigset_t = 0;
            unsafe { libc::sigprocmask(libc::SIG_BLOCK, std::ptr::null(), &mut mask) };
            say(format!("defaults {defaults} mask {mask}"));
            ExitCode::SUCCESS
        }
        // Leave a descendant behind in the group, never waited for.
        Some("linger") => {
            #[allow(clippy::zombie_processes)]
            let child = helper(&["sleep"]).spawn().unwrap();
            say(format!("linger {}", child.id()));
            ExitCode::SUCCESS
        }
        // Leave a descendant that ends by itself after `args[1]` ms.
        Some("linger-for") => {
            #[allow(clippy::zombie_processes)]
            let child = helper(&["sleep-ms", &args[1]]).spawn().unwrap();
            say(format!("linger {}", child.id()));
            ExitCode::SUCCESS
        }
        // A descendant that leaves the group (its own session), which the
        // guard can only know as a child it recorded while the browser ran.
        Some("escape") => {
            let mut command = helper(&["sleep"]);
            // A claimed Chromium cleanup role is diagnostic, never immunity
            // from this guard's forced cleanup of a lingering descendant.
            command.arg("--type=code-sign-clone-cleanup");
            // SAFETY: setsid in the forked child before exec.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            #[allow(clippy::zombie_processes)]
            let child = command.spawn().unwrap();
            say(format!("linger {}", child.id()));
            // Keep the browser alive until the test has read both births.
            // Closing the pipe on test failure also releases this stand-in.
            let mut release = String::new();
            io::stdin().lock().read_line(&mut release).unwrap();
            // Long enough for several of the guard's observations.
            std::thread::sleep(Duration::from_millis(1500));
            ExitCode::SUCCESS
        }
        // A descendant outside the group (its own session) that forks long
        // lived children of its own until it is stopped: the breeder reports
        // on a pipe once its first child exists, and only then does this
        // browser exit (after the guard has had time to record the breeder),
        // so the guard's natural phase and its kill both find it forking.
        Some("breed") => {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([STANDIN, "breeder"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            // SAFETY: setsid in the forked child before exec.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            #[allow(clippy::zombie_processes)]
            let mut child = command.spawn().unwrap();
            let mut line = String::new();
            BufReader::new(child.stdout.take().unwrap())
                .read_line(&mut line)
                .unwrap();
            assert_eq!(line, "forking\n");
            say(format!("breeder {}", child.id()));
            std::thread::sleep(Duration::from_millis(300));
            ExitCode::SUCCESS
        }
        // Forks a child every 5 ms (at most 400, over 2 s, beyond the
        // guard's 0.5 s natural phase), each only waiting for a signal.
        Some("breeder") => {
            for n in 0..400 {
                // SAFETY: this stand-in is single-threaded; the child only
                // waits for a signal and never returns into Rust.
                if unsafe { libc::fork() } == 0 {
                    loop {
                        unsafe { libc::pause() };
                    }
                }
                if n == 0 {
                    say("forking".to_string());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            std::thread::sleep(Duration::from_secs(60));
            ExitCode::SUCCESS
        }
        Some("sleep") => {
            say(format!("ready {}", std::process::id()));
            std::thread::sleep(Duration::from_secs(60));
            ExitCode::SUCCESS
        }
        Some("sleep-ms") => {
            std::thread::sleep(Duration::from_millis(args[1].parse().unwrap()));
            ExitCode::SUCCESS
        }
        other => panic!("unknown stand-in mode {other:?}"),
    }
}

/// The stand-in App: starts the guard (args: receipt, then the stand-in
/// browser's mode), reports its pid and the browser's line, then waits to be
/// killed.
fn app(args: &[String]) -> ExitCode {
    let exe = std::env::current_exe().unwrap();
    let mut command = Command::new(&exe);
    command
        .arg(&args[0])
        .arg(&exe)
        .arg(STANDIN)
        .args(&args[1..])
        .env(AS_GUARD, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .process_group(0);
    // Never waited for: this stand-in App is killed, and the guard outlives
    // it on purpose (the case under test).
    #[allow(clippy::zombie_processes)]
    let mut guard = command.spawn().unwrap();
    let mut lines = BufReader::new(guard.stdout.take().unwrap()).lines();
    say(format!("guard {}", guard.id()));
    say(lines.next().unwrap().unwrap());
    std::thread::sleep(Duration::from_secs(60));
    ExitCode::SUCCESS
}

type Test = (&'static str, fn());
macro_rules! tests {
    ($($name:ident),* $(,)?) => { &[$((stringify!($name), $name as fn())),*] };
}
const TESTS: &[Test] = tests![
    normal_exit_writes_a_complete_normal_receipt,
    a_non_zero_browser_exit_is_reported_and_fails,
    the_browser_keeps_the_pipes_and_default_stop_signals,
    a_lingering_descendant_is_killed_and_counted,
    a_descendant_that_left_the_group_is_still_killed,
    a_descendant_that_ends_by_itself_is_not_forced,
    a_tree_forking_as_the_kill_begins_is_frozen_then_killed,
    an_unwritable_log_still_kills_the_frozen_tree,
    a_stop_request_drains_a_running_browser,
    the_apps_death_ends_the_guard_and_the_browser,
    refusals_create_no_receipt_and_start_nothing,
    a_browser_that_cannot_start_leaves_an_incomplete_receipt,
    pid_reuse_zombie_and_listing_rules,
    wait_status_as_root_exit,
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

/// One case: its directory (kept) and the receipt position.
struct Case {
    receipt: PathBuf,
}

impl Case {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("sumi-gm-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self {
            receipt: dir.join("guard.json"),
        }
    }

    /// The guard with a stand-in browser in `mode`, leading its own group.
    fn guard(&self, mode: &[&str]) -> Command {
        let exe = std::env::current_exe().unwrap();
        let mut command = Command::new(&exe);
        command
            .arg(&self.receipt)
            .arg(&exe)
            .arg(STANDIN)
            .args(mode)
            .env(AS_GUARD, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .process_group(0);
        command
    }

    fn receipt(&self) -> Value {
        serde_json::from_slice(&std::fs::read(&self.receipt).unwrap()).unwrap()
    }
}

/// The first line the stand-in browser wrote on the guard's stdout.
fn first_line(guard: &mut Child) -> String {
    let mut line = String::new();
    BufReader::new(guard.stdout.as_mut().unwrap())
        .read_line(&mut line)
        .unwrap();
    line.trim_end().to_string()
}

fn pid_in(line: &str) -> i32 {
    line.rsplit(' ').next().unwrap().parse().unwrap()
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

/// Whether a pid still names a running (not zombie) process.
fn running(pid: i32) -> bool {
    matches!(facts(pid), Ok(Some(facts)) if !facts.zombie)
}

/// Nothing of the case runs: the given processes have ended and the guard's
/// group is empty. Called after the guard's exit was observed; teardown
/// takes milliseconds, so 3 s stays far below any stand-in's own lifetime
/// (a sleeping stand-in runs 60 s).
fn assert_nothing_left(guard: i32, pids: &[i32]) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let alive: Vec<i32> = pids.iter().copied().filter(|pid| running(*pid)).collect();
        let members: Vec<i32> = group(guard)
            .unwrap_or_default()
            .into_iter()
            .filter(|pid| running(*pid))
            .collect();
        if alive.is_empty() && members.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "left running: {alive:?}; in the guard's group: {members:?}"
        );
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
    let mut guard = case.guard(&["exit", "0"]).spawn().unwrap();
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
    assert_nothing_left(guard.id() as i32, &[]);
}

fn a_non_zero_browser_exit_is_reported_and_fails() {
    let case = Case::new("exit3");
    let mut guard = case.guard(&["exit", "3"]).spawn().unwrap();
    let status = finish(&mut guard, Duration::from_secs(10));
    let proof = case.receipt();
    assert_eq!(status.code(), Some(1), "{proof}");
    assert!(clean(&proof), "{proof}");
    assert_eq!(proof["rootExit"], 3);
    assert_eq!(proof["complete"], true);
    assert_eq!(proof["forcedCleanup"], 0);
}

fn the_browser_keeps_the_pipes_and_default_stop_signals() {
    let case = Case::new("echo");
    let mut command = case.guard(&["echo"]);
    command.stdin(Stdio::piped());
    let mut guard = command.spawn().unwrap();
    guard
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"operation\":\"ping\"}\n")
        .unwrap();
    let mut output = String::new();
    guard
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut output)
        .unwrap();
    let status = finish(&mut guard, Duration::from_secs(10));
    let proof = case.receipt();
    assert_eq!(status.code(), Some(0), "{proof}");
    assert_eq!(output, "{\"operation\":\"ping\"}\ndefaults true mask 0\n");
    assert!(clean(&proof), "{proof}");
}

fn a_lingering_descendant_is_killed_and_counted() {
    let case = Case::new("linger");
    let log = case.receipt.with_extension("log");
    let mut guard = case
        .guard(&["linger"])
        .stderr(File::create(&log).unwrap())
        .spawn()
        .unwrap();
    let helper = pid_in(&first_line(&mut guard));
    let helper_start = facts(helper).unwrap().expect("live lingering helper").start;
    let status = finish(&mut guard, Duration::from_secs(10));
    let proof = case.receipt();
    assert_eq!(status.code(), Some(0), "{proof}");
    assert!(clean(&proof), "{proof}");
    assert_eq!(proof["rootExit"], 0);
    assert_eq!(proof["complete"], true);
    assert_eq!(proof["forcedCleanup"], 1, "{proof}");
    assert!(proof["observed"].as_u64().unwrap() >= 2, "{proof}");
    assert_nothing_left(guard.id() as i32, &[helper]);
    let log = std::fs::read_to_string(log).unwrap();
    print!("{log}");
    let forced: Vec<Value> = log
        .lines()
        .filter_map(|line| line.strip_prefix("takosumi-browser-guard: forced "))
        .map(|line| serde_json::from_str(line).expect("structured forced identity"))
        .collect();
    assert_eq!(forced.len(), 1, "{log}");
    assert_eq!(forced[0]["pid"], helper);
    assert_eq!(forced[0]["start"], helper_start);
    assert_eq!(forced[0]["freezeSnapshot"]["group"], guard.id());
    assert_eq!(forced[0]["freezeSnapshot"]["shownStopped"], true);
    assert!(!forced[0]["freezeSnapshot"]["name"]
        .as_str()
        .unwrap()
        .is_empty());
}

fn a_descendant_that_left_the_group_is_still_killed() {
    let case = Case::new("escape");
    let log = case.receipt.with_extension("log");
    let mut guard = case
        .guard(&["escape"])
        .env(ROLE_DIAGNOSTICS, "1")
        .stdin(Stdio::piped())
        .stderr(File::create(&log).unwrap())
        .spawn()
        .unwrap();
    let helper = pid_in(&first_line(&mut guard));
    let identity = facts(helper).unwrap().expect("live escaped helper");
    let parent_start = facts(identity.ppid)
        .unwrap()
        .expect("live browser parent")
        .start;
    guard.stdin.take().unwrap().write_all(b"release\n").unwrap();
    let status = finish(&mut guard, Duration::from_secs(15));
    let proof = case.receipt();
    let still = running(helper);
    if still {
        // SAFETY: a stand-in this test started; never left behind.
        unsafe { libc::kill(helper, libc::SIGKILL) };
    }
    assert!(!still, "the escaped descendant survived: {proof}");
    assert_eq!(status.code(), Some(0), "{proof}");
    assert!(clean(&proof), "{proof}");
    assert_eq!(proof["forcedCleanup"], 1, "{proof}");
    let log = std::fs::read_to_string(log).unwrap();
    let forced: Vec<Value> = log
        .lines()
        .filter_map(|line| line.strip_prefix("takosumi-browser-guard: forced "))
        .map(|line| serde_json::from_str(line).expect("structured forced identity"))
        .collect();
    assert_eq!(forced.len(), 1, "{log}");
    assert_eq!(forced[0]["pid"], helper);
    assert_eq!(forced[0]["start"], identity.start);
    assert_eq!(forced[0]["firstObserved"]["parent"], identity.ppid);
    assert_eq!(
        forced[0]["firstObserved"]["trackedParentStart"],
        parent_start
    );
    // The first observation may precede the child's pre_exec setsid().
    let first_group = forced[0]["firstObserved"]["group"].as_i64().unwrap();
    assert!(
        first_group == i64::from(helper) || first_group == i64::from(guard.id()),
        "{log}"
    );
    assert_eq!(forced[0]["freezeSnapshot"]["parent"], 1);
    let observation: Value = log
        .lines()
        .find_map(|line| line.strip_prefix("takosumi-browser-guard: cleanup-role "))
        .map(|line| serde_json::from_str(line).unwrap())
        .expect("owned orphan role diagnostic");
    assert_eq!(observation["pid"], helper);
    assert_eq!(observation["start"], identity.start);
    assert_eq!(observation["role"], "code-sign-clone-cleanup");
    assert_eq!(observation["identitiesUnchanged"], true);
    assert_eq!(observation["diagnosticOnly"], true);
}

fn a_descendant_that_ends_by_itself_is_not_forced() {
    let case = Case::new("linger-for");
    let mut guard = case.guard(&["linger-for", "150"]).spawn().unwrap();
    let helper = pid_in(&first_line(&mut guard));
    let status = finish(&mut guard, Duration::from_secs(10));
    let proof = case.receipt();
    assert_eq!(status.code(), Some(0), "{proof}");
    assert!(clean(&proof), "{proof}");
    assert_eq!(proof["forcedCleanup"], 0, "{proof}");
    assert_nothing_left(guard.id() as i32, &[helper]);
}

fn a_tree_forking_as_the_kill_begins_is_frozen_then_killed() {
    let case = Case::new("breed");
    let mut guard = case.guard(&["breed"]).spawn().unwrap();
    let breeder = pid_in(&first_line(&mut guard));
    let status = finish(&mut guard, Duration::from_secs(15));
    let proof = case.receipt();
    // The breeder's children never leave its group (its pid): none of them
    // may outlive the guard, though their parent died with them.
    let deadline = Instant::now() + Duration::from_secs(3);
    let survivors = loop {
        let left: Vec<i32> = group(breeder)
            .unwrap_or_default()
            .into_iter()
            .filter(|pid| running(*pid))
            .collect();
        if left.is_empty() || Instant::now() >= deadline {
            break left;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if !survivors.is_empty() {
        // SAFETY: the stand-ins' own group, still holding them.
        unsafe { libc::killpg(breeder, libc::SIGKILL) };
    }
    assert!(
        survivors.is_empty(),
        "survived the kill: {survivors:?} {proof}"
    );
    assert_eq!(status.code(), Some(0), "{proof}");
    assert!(clean(&proof), "{proof}");
    assert_eq!(proof["rootExit"], 0, "{proof}");
    assert_eq!(proof["complete"], true, "{proof}");
    // Every process seen but the browser (which exited by itself, and
    // counts once) was the breeder or a child of it, none of which ends by
    // itself: each was killed and counted once. The breeder had a child
    // before the browser exited.
    let forced = proof["forcedCleanup"].as_u64().unwrap();
    let observed = proof["observed"].as_u64().unwrap();
    assert!(forced >= 2, "{proof}");
    assert_eq!(forced + 1, observed, "{proof}");
}

fn an_unwritable_log_still_kills_the_frozen_tree() {
    let case = Case::new("no-log");
    // The guard's standard error (the browser.log) is a pipe whose reader
    // is closed: every write fails (EPIPE; Rust programs ignore SIGPIPE).
    // The freeze's line then cannot be written, and the guard must still
    // kill what it stopped and write its receipt.
    let mut ends = [0; 2];
    // SAFETY: pipe fills two descriptors this test then owns.
    assert_eq!(unsafe { libc::pipe(ends.as_mut_ptr()) }, 0);
    let (reader, writer) =
        unsafe { (OwnedFd::from_raw_fd(ends[0]), OwnedFd::from_raw_fd(ends[1])) };
    drop(reader);
    let mut command = case.guard(&["sleep"]);
    command.stderr(Stdio::from(writer));
    let mut guard = command.spawn().unwrap();
    // Drop the command (and its copy of the writer) at once.
    drop(command);
    let browser = pid_in(&first_line(&mut guard));
    // SAFETY: the guard this test started.
    unsafe { libc::kill(guard.id() as i32, libc::SIGTERM) };
    let status = finish(&mut guard, Duration::from_secs(10));
    let proof = case.receipt();
    // Exit 1 (killed browser), not a panic's 101; nothing left stopped.
    assert_eq!(status.code(), Some(1), "{proof}");
    assert_eq!(proof["rootExit"], -libc::SIGKILL, "{proof}");
    assert_eq!(proof["complete"], true, "{proof}");
    assert_eq!(proof["forcedCleanup"], 1, "{proof}");
    assert!(clean(&proof), "{proof}");
    assert_nothing_left(guard.id() as i32, &[browser]);
}

fn a_stop_request_drains_a_running_browser() {
    let case = Case::new("stop");
    let mut guard = case.guard(&["sleep"]).spawn().unwrap();
    let browser = pid_in(&first_line(&mut guard));
    // SAFETY: the guard this test started.
    unsafe { libc::kill(guard.id() as i32, libc::SIGTERM) };
    let status = finish(&mut guard, Duration::from_secs(10));
    let proof = case.receipt();
    assert_eq!(status.code(), Some(1), "{proof}");
    assert_eq!(proof["rootExit"], -libc::SIGKILL, "{proof}");
    assert_eq!(proof["complete"], true, "{proof}");
    assert_eq!(proof["forcedCleanup"], 1, "{proof}");
    assert!(clean(&proof), "{proof}");
    assert_nothing_left(guard.id() as i32, &[browser]);
}

fn the_apps_death_ends_the_guard_and_the_browser() {
    let case = Case::new("app-death");
    let mut app = Command::new(std::env::current_exe().unwrap())
        .arg(APP)
        .arg(&case.receipt)
        .arg("sleep")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(app.stdout.take().unwrap()).lines();
    let guard = pid_in(&lines.next().unwrap().unwrap());
    let browser = pid_in(&lines.next().unwrap().unwrap());
    app.kill().unwrap();
    app.wait().unwrap();
    // The guard (now launchd's child) drains on the App's exit and ends.
    let deadline = Instant::now() + Duration::from_secs(10);
    while running(guard) {
        assert!(Instant::now() < deadline, "the guard outlived its App");
        std::thread::sleep(Duration::from_millis(20));
    }
    // The stand-in browser never reads its input, so only the guard's kill,
    // after the App's exit woke it, can end it: SIGKILL, counted once.
    let proof = case.receipt();
    assert_eq!(proof["rootExit"], -libc::SIGKILL, "{proof}");
    assert_eq!(proof["complete"], true, "{proof}");
    assert_eq!(proof["forcedCleanup"], 1, "{proof}");
    assert!(clean(&proof), "{proof}");
    assert_nothing_left(guard, &[browser]);
}

fn refusals_create_no_receipt_and_start_nothing() {
    let marker_case = Case::new("refusal");
    let exe = std::env::current_exe().unwrap();
    // A relative receipt, an existing one, and a guard that does not lead
    // its own process group.
    let existing = marker_case.receipt.clone();
    std::fs::write(&existing, b"{}").unwrap();
    // The stand-in browser would leave this mark had it started.
    let mark = marker_case.receipt.with_file_name("browser-started");
    for (receipt, own_group) in [
        (PathBuf::from("guard.json"), true),
        (existing.clone(), true),
        (marker_case.receipt.with_file_name("fresh.json"), false),
    ] {
        let mut command = Command::new(&exe);
        command
            .arg(&receipt)
            .arg(&exe)
            .arg(STANDIN)
            .arg("touch")
            .arg(&mark)
            .env(AS_GUARD, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if own_group {
            command.process_group(0);
        }
        let status = command.status().unwrap();
        assert_eq!(status.code(), Some(2), "{receipt:?}");
        if receipt != existing {
            assert!(!receipt.is_absolute() || !receipt.exists(), "{receipt:?}");
        }
        assert!(!mark.exists(), "the browser started: {receipt:?}");
    }
    assert_eq!(std::fs::read(&existing).unwrap(), b"{}");
}

fn a_browser_that_cannot_start_leaves_an_incomplete_receipt() {
    let case = Case::new("missing");
    let mut guard = Command::new(std::env::current_exe().unwrap())
        .arg(&case.receipt)
        .arg("/nonexistent/takosumi-cef-browser-process")
        .env(AS_GUARD, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();
    let status = finish(&mut guard, Duration::from_secs(10));
    let proof = case.receipt();
    assert_eq!(status.code(), Some(1), "{proof}");
    assert_eq!(proof["complete"], false, "{proof}");
    assert!(proof["rootExit"].is_null(), "{proof}");
    assert!(proof["error"].is_string(), "{proof}");
}

fn pid_reuse_zombie_and_listing_rules() {
    // A child found under a parent must not be older than it.
    assert!(plausible_child(2_000, 1_000));
    assert!(plausible_child(1_000, 1_000));
    assert!(!plausible_child(999, 1_000));
    let facts = |start, zombie| Facts {
        ppid: 1,
        pgid: 1,
        start,
        zombie,
        stopped: false,
        name: [0; 32],
    };
    // The same process: same start time and not a zombie.
    assert!(still_running(5, Some(facts(5, false))));
    assert!(!still_running(5, Some(facts(6, false))), "a reused pid");
    assert!(
        !still_running(5, Some(facts(5, true))),
        "a zombie has ended"
    );
    assert!(!still_running(5, None), "gone");
    // A stopped process still runs (the freeze's processes until the kill).
    assert!(still_running(
        5,
        Some(Facts {
            stopped: true,
            ..facts(5, false)
        })
    ));
    // This process's own facts are readable, and it runs.
    let me = std::process::id() as i32;
    let mine = super::facts(me).unwrap().expect("this process's facts");
    assert!(!mine.zombie && !mine.stopped && mine.start > 0);
    // A group listing holds its leader. For a process that leads no group
    // the listing is empty (as libproc also answers an error), which
    // own_group refuses as incomplete.
    let sleeper = |own_group: bool| {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([STANDIN, "sleep"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if own_group {
            command.process_group(0);
        }
        let mut child = command.spawn().unwrap();
        first_line(&mut child);
        child
    };
    let mut leader = sleeper(true);
    let mut member = sleeper(false);
    let listed = own_group(leader.id() as i32);
    let refused = own_group(member.id() as i32);
    for child in [&mut leader, &mut member] {
        child.kill().unwrap();
        child.wait().unwrap();
    }
    assert_eq!(listed.unwrap(), vec![leader.id() as i32]);
    assert!(refused.is_err(), "{refused:?}");
}

fn wait_status_as_root_exit() {
    // W_EXITCODE(3, 0) and a SIGKILL termination, as waitpid reports them.
    assert_eq!(exit_code(3 << 8), 3);
    assert_eq!(exit_code(0), 0);
    assert_eq!(exit_code(libc::SIGKILL), -libc::SIGKILL);
}
