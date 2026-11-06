//! Self-tests for `takosumi-browser-guard` with stand-in browsers.
//!
//! The guard must run as its own single-threaded process (it blocks signals,
//! becomes a subreaper and arms a parent-death signal), which libtest's
//! worker threads cannot host. The bin therefore uses `harness = false`: this
//! runner re-executes the self-test binary with `AS_GUARD` set, so each case
//! drives the same `prepare`/`supervise` code a release build runs.
//! `cargo test --bin takosumi-browser-guard [-- <name filter>]`.

use super::*;
use serde_json::{json, Value};
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::{Child, ExitStatus, Stdio};

pub(crate) const AS_GUARD: &str = "TAKOSUMI_BROWSER_GUARD_SELF_TEST_AS_GUARD";

/// The stand-in browser: `python3 -c STANDIN <case directory> <mode>`.
/// Helpers record `[pid, start time]` so a test can prove they ended.
const STANDIN: &str = r#"
import json,os,signal,sys,time
from pathlib import Path
root=Path(sys.argv[1]); mode=sys.argv[2]
def record(name):
    start=Path('/proc/self/stat').read_text().rsplit(') ',1)[1].split()[19]
    (root/(name+'.tmp')).write_text(json.dumps([os.getpid(),int(start)]))
    os.rename(root/(name+'.tmp'),root/name)
def wait_for(name):
    deadline=time.monotonic()+15
    while not (root/name).exists():
        if time.monotonic()>deadline: os._exit(90)
        time.sleep(0.01)
def helper(term):
    if os.fork()==0:
        os.setsid()
        if term=='late':
            def late(signum,frame):
                signal.signal(signal.SIGTERM,signal.SIG_IGN)
                if os.fork()==0:
                    os.setsid(); record('late.json'); time.sleep(30); os._exit(0)
                os._exit(0)
            signal.signal(signal.SIGTERM,late)
        elif term=='ignore':
            signal.signal(signal.SIGTERM,signal.SIG_IGN)
        record('helper.json'); time.sleep(30); os._exit(0)
    wait_for('helper.json')
if mode=='normal':
    status=dict(line.split(':\t',1) for line in Path('/proc/self/status').read_text().splitlines() if ':\t' in line)
    if int(status['SigBlk'],16) or int(status['SigIgn'],16)&(1<<(signal.SIGTERM-1)): os._exit(91)
    (root/'started').touch(); os._exit(0)
if mode=='exit3': os._exit(3)
if mode=='marker': (root/'ran').touch(); os._exit(0)
if mode=='helper': helper('ignore'); os._exit(0)
if mode=='late': helper('late'); os._exit(0)
if mode=='stop': helper('default'); time.sleep(30); os._exit(0)
if mode=='stubborn':
    signal.signal(signal.SIGTERM,signal.SIG_IGN); helper('ignore'); time.sleep(30); os._exit(0)
if mode=='long': (root/'started').touch(); time.sleep(3.5); os._exit(0)
if mode=='echo':
    sys.stdout.write(sys.stdin.readline()); sys.stdout.flush()
    os._exit(0 if sys.stdin.read()=='' else 92)
os._exit(93)
"#;

/// Starts the guard in its own session, prints its pid, waits for one line on
/// stdin and for the stand-in's helper, then exits without waiting for it.
const INTERMEDIATE: &str = r#"
import os,subprocess,sys,time
from pathlib import Path
root=Path(sys.argv[1])
guard=subprocess.Popen(sys.argv[2:],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,start_new_session=True)
print(guard.pid,flush=True)
sys.stdin.readline()
deadline=time.monotonic()+15
while not (root/'helper.json').exists():
    if time.monotonic()>deadline: os._exit(3)
    time.sleep(0.01)
os._exit(0)
"#;

/// The receipt contract's fields, sorted.
const RECEIPT_KEYS: [&str; 8] = [
    "cleanupErrors",
    "complete",
    "error",
    "forcedCleanup",
    "kernelNoChildren",
    "observed",
    "remaining",
    "rootExit",
];

type Test = (&'static str, fn());
const TESTS: &[Test] = &[
    (
        "normal_exit_writes_a_complete_normal_receipt",
        normal_exit_writes_a_complete_normal_receipt,
    ),
    (
        "a_non_zero_browser_exit_is_reported_and_fails",
        a_non_zero_browser_exit_is_reported_and_fails,
    ),
    (
        "a_detached_helper_is_reaped_and_counted_as_forced_cleanup",
        a_detached_helper_is_reaped_and_counted_as_forced_cleanup,
    ),
    (
        "a_late_fork_during_the_drain_is_reobserved_until_echild",
        a_late_fork_during_the_drain_is_reobserved_until_echild,
    ),
    (
        "sigterm_to_the_guard_drains_the_browser_and_its_helper",
        sigterm_to_the_guard_drains_the_browser_and_its_helper,
    ),
    (
        "sigint_to_the_guard_drains_the_browser_and_its_helper",
        sigint_to_the_guard_drains_the_browser_and_its_helper,
    ),
    (
        "a_sigterm_ignoring_browser_is_killed_within_the_bound",
        a_sigterm_ignoring_browser_is_killed_within_the_bound,
    ),
    (
        "parent_death_stops_the_guard_and_drains",
        parent_death_stops_the_guard_and_drains,
    ),
    (
        "no_lifetime_cap_and_no_busy_polling_while_the_browser_runs",
        no_lifetime_cap_and_no_busy_polling_while_the_browser_runs,
    ),
    (
        "the_browser_keeps_the_control_pipe_on_stdin_and_stdout",
        the_browser_keeps_the_control_pipe_on_stdin_and_stdout,
    ),
    (
        "an_existing_receipt_or_link_is_refused_before_the_browser_starts",
        an_existing_receipt_or_link_is_refused_before_the_browser_starts,
    ),
    (
        "a_relative_receipt_is_refused_before_the_browser_starts",
        a_relative_receipt_is_refused_before_the_browser_starts,
    ),
    (
        "a_guard_outside_its_own_process_group_is_refused",
        a_guard_outside_its_own_process_group_is_refused,
    ),
    (
        "a_missing_program_argument_is_refused",
        a_missing_program_argument_is_refused,
    ),
    (
        "a_browser_that_cannot_start_leaves_an_incomplete_receipt",
        a_browser_that_cannot_start_leaves_an_incomplete_receipt,
    ),
    (
        "ownership_requires_a_stable_chain_to_this_exact_owner",
        ownership_requires_a_stable_chain_to_this_exact_owner,
    ),
    (
        "the_tracker_retains_every_descendant_with_its_start_time",
        the_tracker_retains_every_descendant_with_its_start_time,
    ),
    (
        "the_stat_parser_uses_the_final_name_delimiter",
        the_stat_parser_uses_the_final_name_delimiter,
    ),
];

/// Runs the selected tests one at a time (each owns its children while it
/// runs). Arguments without a leading `-` filter test names by substring.
/// Selecting nothing is a failure, never an empty pass.
pub(crate) fn run() -> ExitCode {
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
        let outcome = std::panic::catch_unwind(test);
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

fn pause(millis: u64) {
    std::thread::sleep(Duration::from_millis(millis));
}

fn guard_executable() -> PathBuf {
    std::env::current_exe().unwrap()
}

/// The same predicate the App applies (`require_normal_guard`).
fn normal(proof: &Value) -> bool {
    proof["rootExit"] == 0
        && proof["complete"] == true
        && proof["kernelNoChildren"] == true
        && proof["remaining"] == json!([])
        && proof["forcedCleanup"] == 0
        && proof.get("error") == Some(&Value::Null)
        && proof["cleanupErrors"] == json!([])
}

/// One fresh private directory per case. A failed case keeps it for
/// diagnosis; a passed case removes only the directory it created. Recorded
/// helpers that still exist are killed through a pidfd of that exact process.
struct Case {
    dir: PathBuf,
}

impl Case {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sumi-guard-{}-{name}", std::process::id()));
        std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn receipt(&self) -> PathBuf {
        self.path("receipt.json")
    }

    /// The guard (this binary acting as one) around the stand-in in `mode`,
    /// not yet in its own process group.
    fn command(&self, receipt: impl AsRef<OsStr>, mode: &str) -> Command {
        let mut command = Command::new(guard_executable());
        command
            .env(AS_GUARD, "1")
            .arg(receipt)
            .args(["python3", "-I", "-B", "-c", STANDIN])
            .arg(&self.dir)
            .arg(mode)
            .stdin(Stdio::null())
            .stdout(Stdio::null());
        command
    }

    fn start(&self, mode: &str) -> Guarded {
        let mut command = self.command(self.receipt(), mode);
        command.process_group(0);
        Guarded(command.spawn().unwrap())
    }

    /// The receipt, which must carry exactly the contract's fields.
    fn proof(&self) -> Value {
        let proof: Value = serde_json::from_slice(&std::fs::read(self.receipt()).unwrap()).unwrap();
        let mut keys: Vec<&str> = proof
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, RECEIPT_KEYS, "{proof}");
        proof
    }

    fn wait_for(&self, name: &str) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !self.path(name).exists() {
            assert!(Instant::now() < deadline, "{name} never appeared");
            pause(10);
        }
    }

    fn record(&self, name: &str) -> Option<(i32, u64)> {
        let bytes = std::fs::read(self.path(name)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// The recorded process no longer exists (its pid is gone or reused).
    fn ended(&self, name: &str) -> bool {
        let (pid, start) = self.record(name).expect("recorded helper");
        start_time(pid).unwrap() != Some(start)
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        for name in ["helper.json", "late.json"] {
            if let Some((pid, start)) = self.record(name) {
                if let Ok(fd) = pidfd_open(pid) {
                    if start_time(pid) == Ok(Some(start)) {
                        let _ = pidfd_signal(&fd, libc::SIGKILL);
                    }
                }
            }
        }
        if std::thread::panicking() {
            eprintln!("kept failed case {}", self.dir.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// A started guard. A failing test still asks it to stop (and drain).
struct Guarded(Child);

impl Guarded {
    fn pid(&self) -> i32 {
        self.0.id() as i32
    }

    /// The guard is this test's unreaped child, so its pid is still its own.
    fn signal(&self, signal: libc::c_int) {
        assert_eq!(unsafe { libc::kill(self.pid(), signal) }, 0);
    }

    fn wait(&mut self, limit: Duration) -> ExitStatus {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "the guard did not exit");
            pause(10);
        }
    }
}

impl Drop for Guarded {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            unsafe { libc::kill(self.pid(), libc::SIGTERM) };
            let deadline = Instant::now() + Duration::from_secs(6);
            while Instant::now() < deadline && matches!(self.0.try_wait(), Ok(None)) {
                pause(20);
            }
            if matches!(self.0.try_wait(), Ok(None)) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
}

/// An unrelated process in its own process group; the guard must not touch it.
struct Unrelated(Child);

impl Unrelated {
    fn start() -> Self {
        Self(
            Command::new("sleep")
                .arg("30")
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }

    fn assert_alive(&mut self) {
        assert!(
            self.0.try_wait().unwrap().is_none(),
            "an unrelated process was signalled"
        );
    }
}

impl Drop for Unrelated {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn normal_exit_writes_a_complete_normal_receipt() {
    let case = Case::new("normal");
    let status = case.start("normal").wait(Duration::from_secs(20));
    let proof = case.proof();
    // Exit 91 from the stand-in would mean it inherited a blocked or ignored
    // SIGTERM from the guard's signalfd setup.
    assert_eq!(status.code(), Some(0), "{proof}");
    assert!(case.path("started").exists());
    assert!(normal(&proof), "{proof}");
    let mode = std::fs::metadata(case.receipt())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

fn a_non_zero_browser_exit_is_reported_and_fails() {
    let case = Case::new("exit3");
    let status = case.start("exit3").wait(Duration::from_secs(20));
    let proof = case.proof();
    assert_eq!(status.code(), Some(1), "{proof}");
    assert_eq!(proof["rootExit"], 3);
    assert_eq!(proof["complete"], true);
    assert_eq!(proof["kernelNoChildren"], true);
    assert_eq!(proof["forcedCleanup"], 0);
    assert_eq!(proof["error"], Value::Null);
    assert!(!normal(&proof));
}

fn a_detached_helper_is_reaped_and_counted_as_forced_cleanup() {
    let case = Case::new("helper");
    let mut other = Unrelated::start();
    let status = case.start("helper").wait(Duration::from_secs(20));
    let proof = case.proof();
    // As in guard.py, a clean root exit with forced cleanup still exits 0;
    // the forced count is what makes the App treat it as abnormal.
    assert_eq!(status.code(), Some(0), "{proof}");
    assert_eq!(proof["rootExit"], 0);
    assert_eq!(proof["complete"], true, "{proof}");
    assert_eq!(proof["kernelNoChildren"], true);
    assert_eq!(proof["remaining"], json!([]));
    assert!(proof["observed"].as_u64().unwrap() >= 2, "{proof}");
    assert!(proof["forcedCleanup"].as_u64().unwrap() >= 1, "{proof}");
    assert_eq!(proof["error"], Value::Null);
    assert_eq!(proof["cleanupErrors"], json!([]));
    assert!(!normal(&proof));
    assert!(case.ended("helper.json"), "the detached helper survived");
    other.assert_alive();
}

fn a_late_fork_during_the_drain_is_reobserved_until_echild() {
    let case = Case::new("late");
    let status = case.start("late").wait(Duration::from_secs(20));
    let proof = case.proof();
    assert_eq!(status.code(), Some(0), "{proof}");
    assert!(case.path("late.json").exists(), "the late fork never ran");
    assert_eq!(proof["complete"], true, "{proof}");
    assert_eq!(proof["kernelNoChildren"], true);
    assert_eq!(proof["remaining"], json!([]));
    assert!(proof["observed"].as_u64().unwrap() >= 3, "{proof}");
    assert!(proof["forcedCleanup"].as_u64().unwrap() >= 1, "{proof}");
    assert!(case.ended("helper.json"));
    assert!(case.ended("late.json"), "the late-adopted helper survived");
}

fn stop_with(signal: libc::c_int, name: &str) {
    let case = Case::new(name);
    let mut other = Unrelated::start();
    let mut guard = case.start("stop");
    case.wait_for("helper.json");
    let sent = Instant::now();
    guard.signal(signal);
    let status = guard.wait(Duration::from_secs(10));
    let elapsed = sent.elapsed();
    let proof = case.proof();
    assert_eq!(status.code(), Some(1), "{proof}");
    assert_eq!(proof["rootExit"], -libc::SIGTERM, "{proof}");
    assert_eq!(proof["complete"], true);
    assert_eq!(proof["kernelNoChildren"], true);
    assert_eq!(proof["remaining"], json!([]));
    assert!(proof["forcedCleanup"].as_u64().unwrap() >= 1, "{proof}");
    assert_eq!(proof["error"], Value::Null);
    assert_eq!(proof["cleanupErrors"], json!([]));
    assert!(case.ended("helper.json"), "the helper survived the stop");
    other.assert_alive();
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
}

fn sigterm_to_the_guard_drains_the_browser_and_its_helper() {
    stop_with(libc::SIGTERM, "sigterm");
}

fn sigint_to_the_guard_drains_the_browser_and_its_helper() {
    stop_with(libc::SIGINT, "sigint");
}

fn a_sigterm_ignoring_browser_is_killed_within_the_bound() {
    let case = Case::new("stubborn");
    let mut guard = case.start("stubborn");
    case.wait_for("helper.json");
    let sent = Instant::now();
    guard.signal(libc::SIGTERM);
    let status = guard.wait(Duration::from_secs(10));
    let elapsed = sent.elapsed();
    let proof = case.proof();
    assert_eq!(status.code(), Some(1), "{proof}");
    assert_eq!(proof["rootExit"], -libc::SIGKILL, "{proof}");
    assert_eq!(proof["complete"], true);
    assert_eq!(proof["kernelNoChildren"], true);
    assert_eq!(proof["remaining"], json!([]));
    assert!(proof["forcedCleanup"].as_u64().unwrap() >= 2, "{proof}");
    assert!(case.ended("helper.json"));
    // 0.5 s for a natural exit and 0.5 s of SIGTERM come first; SIGKILL then
    // ends both within its 2 s phase.
    assert!(elapsed >= Duration::from_millis(900), "{elapsed:?}");
    assert!(elapsed < Duration::from_millis(3500), "{elapsed:?}");
}

fn parent_death_stops_the_guard_and_drains() {
    let case = Case::new("parent");
    let mut intermediate = Command::new("python3")
        .args(["-I", "-B", "-c", INTERMEDIATE])
        .arg(&case.dir)
        .arg(guard_executable())
        .arg(case.receipt())
        .args(["python3", "-I", "-B", "-c", STANDIN])
        .arg(&case.dir)
        .arg("stop")
        .env(AS_GUARD, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(intermediate.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    // The intermediate is still alive and has not reaped the guard, so this
    // pidfd names that guard.
    let guard = pidfd_open(line.trim().parse().unwrap()).unwrap();
    std::io::Write::write_all(intermediate.stdin.as_mut().unwrap(), b"go\n").unwrap();
    assert_eq!(intermediate.wait().unwrap().code(), Some(0));
    let mut exited = libc::pollfd {
        fd: guard.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd; a pidfd turns readable when its process ends.
    let ready = unsafe { libc::poll(&mut exited, 1, 10_000) };
    if ready != 1 {
        let _ = pidfd_signal(&guard, libc::SIGTERM);
        panic!("the guard kept running after its parent ended");
    }
    let proof = case.proof();
    assert_eq!(proof["rootExit"], -libc::SIGTERM, "{proof}");
    assert_eq!(proof["complete"], true);
    assert_eq!(proof["kernelNoChildren"], true);
    assert_eq!(proof["remaining"], json!([]));
    assert!(case.ended("helper.json"));
}

fn voluntary_switches(pid: i32) -> u64 {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("voluntary_ctxt_switches:"))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

fn no_lifetime_cap_and_no_busy_polling_while_the_browser_runs() {
    let case = Case::new("long");
    let started = Instant::now();
    let mut guard = case.start("long");
    case.wait_for("started");
    let before = voluntary_switches(guard.pid());
    pause(2500);
    let woken = voluntary_switches(guard.pid()) - before;
    let status = guard.wait(Duration::from_secs(20));
    let proof = case.proof();
    // The stand-in lives 3.5 s and must end on its own, normally.
    assert!(started.elapsed() >= Duration::from_millis(3500));
    assert_eq!(status.code(), Some(0), "{proof}");
    assert!(normal(&proof), "{proof}");
    // The 250 ms tick wakes about 10 times in 2.5 s; 20 ms polling would
    // wake about 125 times.
    assert!(woken < 40, "{woken} wakeups in 2.5 s");
}

fn the_browser_keeps_the_control_pipe_on_stdin_and_stdout() {
    let case = Case::new("pipe");
    let mut command = case.command(case.receipt(), "echo");
    command
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let mut guard = Guarded(command.spawn().unwrap());
    let mut input = guard.0.stdin.take().unwrap();
    let mut output = BufReader::new(guard.0.stdout.take().unwrap());
    std::io::Write::write_all(&mut input, b"ping\n").unwrap();
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    assert_eq!(line, "ping\n");
    drop(input); // EOF ends the stand-in normally.
    let status = guard.wait(Duration::from_secs(20));
    let mut rest = String::new();
    output.read_to_string(&mut rest).unwrap();
    assert_eq!(rest, "", "the guard wrote to the browser's stdout");
    let proof = case.proof();
    assert_eq!(status.code(), Some(0), "{proof}");
    assert!(normal(&proof), "{proof}");
}

/// The guard exits 2 and the stand-in never runs.
fn refused(case: &Case, mut command: Command) {
    assert_eq!(command.status().unwrap().code(), Some(2));
    pause(300);
    assert!(!case.path("ran").exists(), "the browser started");
}

fn an_existing_receipt_or_link_is_refused_before_the_browser_starts() {
    let case = Case::new("existing");
    std::fs::write(case.receipt(), "keep").unwrap();
    let mut command = case.command(case.receipt(), "marker");
    command.process_group(0);
    refused(&case, command);
    assert_eq!(std::fs::read_to_string(case.receipt()).unwrap(), "keep");
    // A dangling link is refused too, and its target is never created.
    let link = case.path("link.json");
    std::os::unix::fs::symlink(case.path("absent"), &link).unwrap();
    let mut command = case.command(&link, "marker");
    command.process_group(0);
    refused(&case, command);
    assert!(!case.path("absent").exists());
}

fn a_relative_receipt_is_refused_before_the_browser_starts() {
    let case = Case::new("relative");
    let mut command = case.command("receipt.json", "marker");
    command.process_group(0).current_dir(&case.dir);
    refused(&case, command);
    assert!(!case.receipt().exists());
}

fn a_guard_outside_its_own_process_group_is_refused() {
    let case = Case::new("group");
    // Without process_group(0) the guard shares the test runner's group.
    refused(&case, case.command(case.receipt(), "marker"));
    assert!(!case.receipt().exists());
}

fn a_missing_program_argument_is_refused() {
    let case = Case::new("usage");
    let mut command = Command::new(guard_executable());
    command
        .env(AS_GUARD, "1")
        .arg(case.receipt())
        .process_group(0);
    refused(&case, command);
    assert!(!case.receipt().exists());
}

fn a_browser_that_cannot_start_leaves_an_incomplete_receipt() {
    let case = Case::new("absent");
    let status = Command::new(guard_executable())
        .env(AS_GUARD, "1")
        .arg(case.receipt())
        .arg(case.path("absent-browser"))
        .process_group(0)
        .status()
        .unwrap();
    let proof = case.proof();
    assert_eq!(status.code(), Some(1), "{proof}");
    assert_eq!(proof["complete"], false);
    assert_eq!(proof["rootExit"], Value::Null);
    assert_eq!(proof["error"], "NotFound");
    assert_eq!(proof["kernelNoChildren"], true);
}

fn ownership_requires_a_stable_chain_to_this_exact_owner() {
    // SAFETY: getpid has no preconditions.
    let me = unsafe { libc::getpid() };
    let my_start = start_time(me).unwrap().unwrap();
    let mut mine = Command::new("sleep").arg("30").spawn().unwrap();
    let mut sibling = Command::new("sleep").arg("30").spawn().unwrap();
    let (mine_pid, sibling_pid) = (mine.id() as i32, sibling.id() as i32);
    let mine_start = start_time(mine_pid).unwrap().unwrap();
    let results = (
        is_owned(mine_pid, me, my_start),
        // The same pid with another start time is a different (reused) owner.
        is_owned(mine_pid, me, my_start + 1),
        // A process from another ancestry, as a stale snapshot might name it.
        is_owned(sibling_pid, mine_pid, mine_start),
    );
    for child in [&mut mine, &mut sibling] {
        child.kill().unwrap();
        child.wait().unwrap();
    }
    assert_eq!(results, (Ok(true), Ok(false), Ok(false)));
}

fn the_tracker_retains_every_descendant_with_its_start_time() {
    // SAFETY: getpid has no preconditions.
    let me = unsafe { libc::getpid() };
    let mut shell = Command::new("sh")
        .args(["-c", "sleep 30 & echo $!; wait"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(shell.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let (shell_pid, grandchild) = (shell.id() as i32, line.trim().parse::<i32>().unwrap());
    let expected = [shell_pid, grandchild].map(|pid| (pid, start_time(pid).unwrap().unwrap()));
    let mut tracker = Tracker::default();
    let observed = tracker.observe(me);
    let retained: Vec<(i32, u64)> = tracker
        .handles
        .iter()
        .map(|(pid, (start, _))| (*pid, *start))
        .collect();
    // The test process is no subreaper: end the grandchild first, through
    // the pidfd the tracker retained for it.
    if let Some((_, fd)) = tracker.handles.get(&grandchild) {
        let _ = pidfd_signal(fd, libc::SIGKILL);
    }
    let _ = shell.kill();
    shell.wait().unwrap();
    observed.unwrap();
    assert_eq!(
        retained, expected,
        "only the shell and its child are below the runner"
    );
}

fn the_stat_parser_uses_the_final_name_delimiter() {
    let real = std::fs::read_to_string("/proc/self/stat").unwrap();
    let fields = real.rsplit_once(") ").unwrap().1;
    let words: Vec<&str> = fields.split_whitespace().collect();
    let expected = (words[19].parse().unwrap(), words[1].parse().unwrap());
    assert_eq!(parse_stat(&real), Some(expected));
    // A process name may itself contain ") "; it must not forge the parent
    // or the start time.
    assert_eq!(
        parse_stat(&format!("42 (x) S 999 ) {fields}")),
        Some(expected)
    );
}
