//! Self-tests for `takosumi-browser-guard` with stand-in browsers.
//!
//! The guard must run as its own single-threaded process (it blocks signals,
//! becomes a subreaper and arms a parent-death signal), which libtest's
//! worker threads cannot host. The bin therefore uses `harness = false`: this
//! runner re-executes the self-test binary with `AS_GUARD` set, so each case
//! drives the same `prepare`/`supervise` code a release build runs.
//! `cargo test --bin takosumi-browser-guard [-- <name filter>]`.
//!
//! Stand-ins report `<name> <pid> <start>` lines on the guard's stdout, a
//! pipe, so nothing timing-sensitive waits on disk I/O. Kill bounds are
//! measured on the stand-ins' own pidfds, not on the guard's exit.

use super::*;
use serde_json::{json, Value};
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::{Child, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

pub(crate) const AS_GUARD: &str = "TAKOSUMI_BROWSER_GUARD_SELF_TEST_AS_GUARD";

/// Fault injection for the guard under test: `TAKOSUMI_GUARD_FAULT_<POINT>`
/// set to a number fails that many calls at the point, `always` every call.
pub(crate) fn inject(point: &str) -> bool {
    static USED: Mutex<BTreeMap<String, u32>> = Mutex::new(BTreeMap::new());
    let Ok(value) = std::env::var(format!("TAKOSUMI_GUARD_FAULT_{point}")) else {
        return false;
    };
    if value == "always" {
        return true;
    }
    let mut used = USED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let count = used.entry(point.to_string()).or_insert(0);
    *count += 1;
    *count <= value.parse().unwrap_or(0)
}

/// The stand-in browser: `python3 -c STANDIN <case directory> <mode> [...]`.
const STANDIN: &str = r#"
import json,os,resource,select,signal,sys,time
from pathlib import Path
root=Path(sys.argv[1]); mode=sys.argv[2]
def start(pid):
    return Path(f'/proc/{pid}/stat').read_text().rsplit(') ',1)[1].split()[19]
def record(name,pid=None):
    pid=pid or os.getpid()
    os.write(1,f'{name} {pid} {start(pid)}\n'.encode())
def helper(term):
    ready,done=os.pipe()
    if os.fork()==0:
        os.setsid()
        if term=='late':
            def late(signum,frame):
                signal.signal(signal.SIGTERM,signal.SIG_IGN)
                pid=os.fork()
                if pid==0:
                    os.setsid(); time.sleep(30); os._exit(0)
                record('late',pid); os._exit(0)
            signal.signal(signal.SIGTERM,late)
        elif term=='ignore':
            signal.signal(signal.SIGTERM,signal.SIG_IGN)
        record('helper'); os.write(done,b'1'); time.sleep(30); os._exit(0)
    if not select.select([ready],[],[],15)[0]: os._exit(90)
def released():
    deadline=time.monotonic()+15
    while not (root/'release').exists():
        if time.monotonic()>deadline: os._exit(94)
        time.sleep(0.01)
if mode=='normal':
    status=dict(line.split(':\t',1) for line in Path('/proc/self/status').read_text().splitlines() if ':\t' in line)
    ignored=int(status['SigIgn'],16)
    if int(status['SigBlk'],16) or any(ignored&(1<<(s-1)) for s in (signal.SIGTERM,signal.SIGINT,signal.SIGCHLD)): os._exit(91)
    os._exit(0)
if mode=='exit3': os._exit(3)
if mode=='marker': (root/'ran').touch(); os._exit(0)
if mode=='echo':
    sys.stdout.write(sys.stdin.readline()); sys.stdout.flush()
    os._exit(0 if sys.stdin.read()=='' else 92)
record('root')
if mode=='helper': helper('ignore'); os._exit(0)
if mode=='late': helper('late'); os._exit(0)
if mode=='stop': helper('default'); time.sleep(30); os._exit(0)
if mode=='stubborn':
    signal.signal(signal.SIGTERM,signal.SIG_IGN); helper('ignore'); time.sleep(30); os._exit(0)
if mode=='long': time.sleep(3.5); os._exit(0)
if mode=='sleep': time.sleep(30); os._exit(0)
if mode=='fds':
    def is_open(fd):
        try: os.fstat(fd); return True
        except OSError: return False
    found=[fd for fd in range(3,256) if is_open(fd)]
    (root/'fds.json').write_text(json.dumps([found,*resource.getrlimit(resource.RLIMIT_NOFILE)]))
    record('fds'); released(); os._exit(0)
if mode=='churn':
    count,live,pace=int(sys.argv[3]),float(sys.argv[4]),float(sys.argv[5])
    alive=set()
    for _ in range(count):
        pid=os.fork()
        if pid==0:
            time.sleep(live); os._exit(0)
        alive.add(pid); time.sleep(pace)
        while alive:
            done,_=os.waitpid(-1,os.WNOHANG)
            if not done: break
            alive.discard(done)
    for pid in alive: os.waitpid(pid,0)
    if len(sys.argv)>6: record('held'); released()
    os._exit(0)
os._exit(93)
"#;

/// Starts the guard in its own session with this stdout, records its pid,
/// waits for one line on stdin, then exits without waiting for the guard.
const INTERMEDIATE: &str = r#"
import os,subprocess,sys
guard=subprocess.Popen(sys.argv[1:],stdin=subprocess.DEVNULL,start_new_session=True)
os.write(1,f'guard {guard.pid} 0\n'.encode())
sys.stdin.readline()
os._exit(0)
"#;

/// The receipt contract's fields, sorted.
const RECEIPT_KEYS: [&str; 9] = [
    "cleanupErrors",
    "complete",
    "error",
    "forcedCleanup",
    "kernelNoChildren",
    "nonFatalErrors",
    "observed",
    "remaining",
    "rootExit",
];

type Test = (&'static str, fn());
macro_rules! tests {
    ($($name:ident),* $(,)?) => { &[$((stringify!($name), $name as fn())),*] };
}
const TESTS: &[Test] = tests![
    guard_comm_class_projection_is_private_optional_and_bounded,
    guard_comm_class_owned_names_do_not_change_forced_cleanup,
    guard_readiness_observation_preserves_errno_optional_and_budget,
    terminal_signal_owned_fd_open_failure_cleans_partial_child,
    terminal_signal_readiness_unknown_keeps_eligibility,
    terminal_signal_owned_no_direct_fd_preserves_pid_fallback,
    terminal_signal_owned_terminal_unreaped_child_is_not_forced,
    terminal_signal_owned_live_handler_exit_zero_stays_forced,
    stop_observation_is_bounded_and_uses_only_existing_results,
    normal_exit_writes_a_complete_normal_receipt,
    a_non_zero_browser_exit_is_reported_and_fails,
    a_detached_helper_is_reaped_and_counted_as_forced_cleanup,
    a_late_fork_during_the_drain_is_reobserved_until_echild,
    sigterm_to_the_guard_drains_the_browser_and_its_helper,
    sigint_to_the_guard_drains_the_browser_and_its_helper,
    a_stop_still_drains_when_the_guard_started_with_signals_ignored,
    a_sigterm_ignoring_browser_is_killed_within_the_bound,
    parent_death_stops_the_guard_and_drains,
    no_lifetime_cap_and_no_busy_polling_while_the_browser_runs,
    many_short_lived_descendants_do_not_exhaust_descriptors,
    an_observation_failure_while_running_does_not_stop_the_browser,
    pruning_needs_no_free_descriptor,
    transient_reap_and_wait_failures_do_not_stop_the_browser,
    persistent_wait_failures_drain_after_the_bound,
    a_missing_browser_pidfd_does_not_stop_the_browser,
    without_any_pidfd_the_drain_still_signals_the_browser,
    an_inherited_sigchld_ignore_keeps_the_browser_exit_status,
    the_browser_gets_only_stdio_and_the_original_descriptor_limit,
    the_browser_keeps_the_control_pipe_on_stdin_and_stdout,
    a_pending_stop_starts_no_browser,
    an_existing_receipt_or_link_is_refused_before_the_browser_starts,
    a_relative_receipt_is_refused_before_the_browser_starts,
    a_guard_outside_its_own_process_group_is_refused,
    a_missing_program_argument_is_refused,
    a_browser_that_cannot_start_leaves_an_incomplete_receipt,
    ownership_requires_a_stable_chain_to_this_exact_owner,
    the_tracker_retains_every_descendant_with_its_start_time,
    the_tracker_releases_handles_of_ended_processes,
    the_stat_parser_uses_the_final_name_delimiter,
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

fn pause(millis: u64) {
    std::thread::sleep(Duration::from_millis(millis));
}

fn guard_executable() -> PathBuf {
    std::env::current_exe().unwrap()
}

/// Runs `setup` in the forked guard before exec.
fn at_start(command: &mut Command, setup: impl FnMut() -> io::Result<()> + Send + Sync + 'static) {
    // SAFETY: every setup below makes only async-signal-safe system calls.
    unsafe {
        command.pre_exec(setup);
    }
}

fn check(result: libc::c_int) -> io::Result<()> {
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn descriptor_limit(soft: u64, hard: u64) -> impl FnMut() -> io::Result<()> + Send + Sync {
    move || {
        let limit = libc::rlimit64 {
            rlim_cur: soft,
            rlim_max: hard,
        };
        check(unsafe { libc::setrlimit64(libc::RLIMIT_NOFILE, &limit) })
    }
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

/// Normal, and no observation failed while the browser ran.
fn clean(proof: &Value) -> bool {
    normal(proof) && proof["nonFatalErrors"] == json!([])
}

/// The recorded process no longer exists (its pid is gone or reused).
fn ended((pid, start): (i32, u64)) -> bool {
    start_time(pid).unwrap() != Some(start)
}

/// A pidfd for a recorded process that still exists, verified like the guard
/// does: the start time read after opening must match the record.
fn open_record((pid, start): (i32, u64)) -> Option<OwnedFd> {
    let fd = pidfd_open(pid).ok()?;
    (start_time(pid) == Ok(Some(start))).then_some(fd)
}

#[derive(Default)]
struct Recorded {
    lines: Vec<(String, i32, u64)>,
    closed: bool,
}

/// `<name> <pid> <start>` lines read from a stand-in pipe until it closes.
#[derive(Clone, Default)]
struct Records(Arc<(Mutex<Recorded>, Condvar)>);

impl Records {
    fn collect(stream: impl Read + Send + 'static) -> Self {
        let records = Self::default();
        let shared = records.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                let words: Vec<&str> = line.split_whitespace().collect();
                if let [name, pid, start] = words[..] {
                    if let (Ok(pid), Ok(start)) = (pid.parse(), start.parse()) {
                        shared.state().lines.push((name.to_string(), pid, start));
                        shared.0 .1.notify_all();
                    }
                }
            }
            shared.state().closed = true;
            shared.0 .1.notify_all();
        });
        records
    }

    fn state(&self) -> MutexGuard<'_, Recorded> {
        self.0
             .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn all(&self) -> Vec<(i32, u64)> {
        self.state()
            .lines
            .iter()
            .map(|(_, pid, start)| (*pid, *start))
            .collect()
    }

    /// The named record; fails once the pipe closed or 15 s passed without it.
    fn wait(&self, name: &str) -> (i32, u64) {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut state = self.state();
        loop {
            let found = state
                .lines
                .iter()
                .find(|(recorded, ..)| recorded == name)
                .map(|(_, pid, start)| (*pid, *start));
            if let Some(found) = found {
                return found;
            }
            let now = Instant::now();
            if state.closed || now >= deadline {
                drop(state);
                panic!("{name} was never recorded");
            }
            state = self
                .0
                 .1
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
    }
}

/// Kills every recorded stand-in that still exists, through its own pidfd.
struct Stragglers(Records);

impl Drop for Stragglers {
    fn drop(&mut self) {
        for record in self.0.all() {
            if let Some(fd) = open_record(record) {
                let _ = pidfd_signal(&fd, libc::SIGKILL);
            }
        }
    }
}

/// Pidfds of recorded processes, taken while they are alive.
struct Watch(Vec<OwnedFd>);

impl Watch {
    fn new(records: &[(i32, u64)]) -> Self {
        Self(
            records
                .iter()
                .map(|record| open_record(*record).expect("the recorded process is alive"))
                .collect(),
        )
    }

    /// When the last watched process ended, if all ended before `deadline`.
    fn last_end(&self, deadline: Instant) -> Option<Instant> {
        let mut pending: Vec<RawFd> = self.0.iter().map(AsRawFd::as_raw_fd).collect();
        let mut last = Instant::now();
        while !pending.is_empty() {
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let mut ready: Vec<libc::pollfd> = pending
                .iter()
                .map(|fd| libc::pollfd {
                    fd: *fd,
                    events: libc::POLLIN,
                    revents: 0,
                })
                .collect();
            let millis = (deadline - now).as_millis() as libc::c_int + 1;
            // SAFETY: a valid pollfd array of the given length.
            if unsafe { libc::poll(ready.as_mut_ptr(), ready.len() as libc::nfds_t, millis) } > 0 {
                last = Instant::now();
                pending = ready
                    .iter()
                    .filter(|entry| entry.revents == 0)
                    .map(|entry| entry.fd)
                    .collect();
            }
        }
        Some(last)
    }
}

/// One fresh private directory per case. A failed case keeps it for
/// diagnosis; a passed case removes only the directory it created.
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

    /// Starts `command` in its own process group, collecting the records.
    fn spawn(&self, mut command: Command) -> Guarded {
        command.process_group(0).stdout(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let records = Records::collect(child.stdout.take().unwrap());
        Guarded { child, records }
    }

    fn start(&self, mode: &str) -> Guarded {
        self.spawn(self.command(self.receipt(), mode))
    }

    /// The receipt, which must carry exactly the contract's fields.
    fn proof(&self) -> Value {
        let proof: Value = serde_json::from_slice(&std::fs::read(self.receipt()).unwrap()).unwrap();
        let mut keys: Vec<&str> = proof
            .as_object()
            .unwrap()
            .keys()
            .filter(|name| name.as_str() != "stopObservation") // Optional diagnostics never change required receipt fields.
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, RECEIPT_KEYS, "{proof}");
        proof
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("kept failed case {}", self.dir.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// A started guard. A failing test still asks it to stop (and drain), then
/// kills any recorded stand-in that still exists.
struct Guarded {
    child: Child,
    records: Records,
}

impl Guarded {
    fn pid(&self) -> i32 {
        self.child.id() as i32
    }

    /// The guard is this test's unreaped child, so its pid is still its own.
    fn signal(&self, signal: libc::c_int) {
        assert_eq!(unsafe { libc::kill(self.pid(), signal) }, 0);
    }

    fn wait(&mut self, limit: Duration) -> ExitStatus {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "the guard did not exit");
            pause(10);
        }
    }
}

impl Drop for Guarded {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            unsafe { libc::kill(self.pid(), libc::SIGTERM) };
            let deadline = Instant::now() + Duration::from_secs(6);
            while Instant::now() < deadline && matches!(self.child.try_wait(), Ok(None)) {
                pause(20);
            }
            if matches!(self.child.try_wait(), Ok(None)) {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
        drop(Stragglers(self.records.clone()));
    }
}

/// A child of this test, killed and reaped however the test ends.
struct Reaped(Child);

impl Reaped {
    fn pid(&self) -> i32 {
        self.0.id() as i32
    }
}

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn sleeper(own_group: bool) -> Reaped {
    let mut command = Command::new("sleep");
    command.arg("30").stdin(Stdio::null()).stdout(Stdio::null());
    if own_group {
        command.process_group(0);
    }
    Reaped(command.spawn().unwrap())
}

/// A process this test does not own directly, killed through a pidfd.
struct KillOnDrop(OwnedFd);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = pidfd_signal(&self.0, libc::SIGKILL);
    }
}

fn assert_unrelated_alive(other: &mut Reaped) {
    assert!(
        other.0.try_wait().unwrap().is_none(),
        "an unrelated process was signalled"
    );
}

fn normal_exit_writes_a_complete_normal_receipt() {
    let case = Case::new("normal");
    let mut command = case.command(case.receipt(), "normal");
    // Whatever the guard is given blocked, the browser starts with none.
    at_start(&mut command, || unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGUSR1);
        libc::sigaddset(&mut set, libc::SIGTERM);
        check(libc::sigprocmask(
            libc::SIG_BLOCK,
            &set,
            std::ptr::null_mut(),
        ))
    });
    let status = case.spawn(command).wait(Duration::from_secs(20));
    let proof = case.proof();
    // Exit 91 from the stand-in means it started with a blocked signal or an
    // ignored SIGTERM, SIGINT or SIGCHLD.
    assert_eq!(status.code(), Some(0), "{proof}");
    assert!(clean(&proof), "{proof}");
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
    let mut other = sleeper(true);
    let mut guard = case.start("helper");
    let status = guard.wait(Duration::from_secs(20));
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
    assert!(ended(guard.records.wait("helper")), "the helper survived");
    assert_unrelated_alive(&mut other);
}

fn a_late_fork_during_the_drain_is_reobserved_until_echild() {
    let case = Case::new("late");
    let mut guard = case.start("late");
    let status = guard.wait(Duration::from_secs(20));
    let proof = case.proof();
    assert_eq!(status.code(), Some(0), "{proof}");
    // The helper records the late child the moment it forks it, on a pipe.
    let late = guard.records.wait("late");
    assert_eq!(proof["complete"], true, "{proof}");
    assert_eq!(proof["kernelNoChildren"], true);
    assert_eq!(proof["remaining"], json!([]));
    assert!(proof["observed"].as_u64().unwrap() >= 3, "{proof}");
    assert!(proof["forcedCleanup"].as_u64().unwrap() >= 1, "{proof}");
    assert!(ended(guard.records.wait("helper")));
    assert!(ended(late), "the late-adopted helper survived");
}

fn stop_with(signal: libc::c_int, name: &str, ignored_at_start: bool) {
    let case = Case::new(name);
    let mut other = sleeper(true);
    let mut command = case.command(case.receipt(), "stop");
    if ignored_at_start {
        at_start(&mut command, || unsafe {
            for ignored in [libc::SIGTERM, libc::SIGINT, libc::SIGCHLD] {
                if libc::signal(ignored, libc::SIG_IGN) == libc::SIG_ERR {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    let mut guard = case.spawn(command);
    let watch = Watch::new(&[guard.records.wait("root"), guard.records.wait("helper")]);
    let sent = Instant::now();
    guard.signal(signal);
    let last = watch.last_end(sent + Duration::from_secs(10));
    let status = guard.wait(Duration::from_secs(20));
    let proof = case.proof();
    assert_eq!(status.code(), Some(1), "{proof}");
    assert_eq!(proof["rootExit"], -libc::SIGTERM, "{proof}");
    assert_eq!(proof["complete"], true);
    assert_eq!(proof["kernelNoChildren"], true);
    assert_eq!(proof["remaining"], json!([]));
    assert!(proof["forcedCleanup"].as_u64().unwrap() >= 1, "{proof}");
    assert_eq!(proof["error"], Value::Null);
    assert_eq!(proof["cleanupErrors"], json!([]));
    assert_unrelated_alive(&mut other);
    // SIGTERM follows the 0.5 s natural phase and ends both.
    let elapsed = last.expect("the browser or its helper survived the stop") - sent;
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
}

fn sigterm_to_the_guard_drains_the_browser_and_its_helper() {
    stop_with(libc::SIGTERM, "sigterm", false);
}

fn sigint_to_the_guard_drains_the_browser_and_its_helper() {
    stop_with(libc::SIGINT, "sigint", false);
}

fn a_stop_still_drains_when_the_guard_started_with_signals_ignored() {
    stop_with(libc::SIGTERM, "ignored", true);
}

fn a_sigterm_ignoring_browser_is_killed_within_the_bound() {
    let case = Case::new("stubborn");
    let mut guard = case.start("stubborn");
    let watch = Watch::new(&[guard.records.wait("root"), guard.records.wait("helper")]);
    let sent = Instant::now();
    guard.signal(libc::SIGTERM);
    let last = watch.last_end(sent + Duration::from_secs(10));
    let status = guard.wait(Duration::from_secs(20));
    let proof = case.proof();
    assert_eq!(status.code(), Some(1), "{proof}");
    assert_eq!(proof["rootExit"], -libc::SIGKILL, "{proof}");
    assert_eq!(proof["complete"], true);
    assert_eq!(proof["kernelNoChildren"], true);
    assert_eq!(proof["remaining"], json!([]));
    assert!(proof["forcedCleanup"].as_u64().unwrap() >= 2, "{proof}");
    // 0.5 s for a natural exit and 0.5 s of SIGTERM come first; SIGKILL then
    // ends both within its 2 s phase.
    let elapsed = last.expect("the browser or its helper survived SIGKILL") - sent;
    assert!(elapsed >= Duration::from_millis(900), "{elapsed:?}");
    assert!(elapsed < Duration::from_millis(3500), "{elapsed:?}");
}

fn parent_death_stops_the_guard_and_drains() {
    let case = Case::new("parent");
    let mut intermediate = Command::new("python3");
    intermediate
        .args(["-I", "-B", "-c", INTERMEDIATE])
        .arg(guard_executable())
        .arg(case.receipt())
        .args(["python3", "-I", "-B", "-c", STANDIN])
        .arg(&case.dir)
        .arg("stop")
        .env(AS_GUARD, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let mut intermediate = Reaped(intermediate.spawn().unwrap());
    let records = Records::collect(intermediate.0.stdout.take().unwrap());
    let (guard_pid, _) = records.wait("guard");
    // The intermediate is alive and has not reaped the guard, so this pidfd
    // names that guard.
    let guard = pidfd_open(guard_pid).unwrap();
    let _stragglers = Stragglers(records.clone());
    let helper = records.wait("helper");
    std::io::Write::write_all(intermediate.0.stdin.as_mut().unwrap(), b"go\n").unwrap();
    assert_eq!(intermediate.0.wait().unwrap().code(), Some(0));
    let mut exited = libc::pollfd {
        fd: guard.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd; a pidfd turns readable when its process ends.
    if unsafe { libc::poll(&mut exited, 1, 10_000) } != 1 {
        let _ = pidfd_signal(&guard, libc::SIGTERM);
        panic!("the guard kept running after its parent ended");
    }
    let proof = case.proof();
    assert_eq!(proof["rootExit"], -libc::SIGTERM, "{proof}");
    assert_eq!(proof["complete"], true);
    assert_eq!(proof["kernelNoChildren"], true);
    assert_eq!(proof["remaining"], json!([]));
    assert!(ended(helper));
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
    guard.records.wait("root");
    let before = voluntary_switches(guard.pid());
    pause(2500);
    let woken = voluntary_switches(guard.pid()) - before;
    let status = guard.wait(Duration::from_secs(20));
    let proof = case.proof();
    // The stand-in lives 3.5 s and must end on its own, normally.
    assert!(started.elapsed() >= Duration::from_millis(3500));
    assert_eq!(status.code(), Some(0), "{proof}");
    assert!(clean(&proof), "{proof}");
    // The 250 ms tick wakes about 10 times in 2.5 s; 20 ms polling would
    // wake about 125 times.
    assert!(woken < 40, "{woken} wakeups in 2.5 s");
}

fn descriptor_count(pid: i32) -> usize {
    std::fs::read_dir(format!("/proc/{pid}/fd"))
        .map(Iterator::count)
        .unwrap_or(0)
}

/// Runs the churn stand-in (300 children, each alive 0.3 s, one every 12 ms)
/// under a descriptor limit of `limit`, and returns the receipt, the guard's
/// exit status and its largest sampled descriptor count.
fn churn(name: &str, limit: u64) -> (Value, ExitStatus, usize) {
    let case = Case::new(name);
    let mut command = case.command(case.receipt(), "churn");
    command.args(["300", "0.3", "0.012"]);
    at_start(&mut command, descriptor_limit(limit, limit));
    let mut guard = case.spawn(command);
    guard.records.wait("root");
    let mut peak = 0;
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = guard.child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the guard did not exit");
        peak = peak.max(descriptor_count(guard.pid()));
        pause(50);
    };
    (case.proof(), status, peak)
}

fn many_short_lived_descendants_do_not_exhaust_descriptors() {
    let (proof, status, peak) = churn("churn", 64);
    assert_eq!(status.code(), Some(0), "{proof}");
    assert!(clean(&proof), "{proof}");
    // Far more owned processes came and went than the 64 descriptors allow
    // at once; only the existing ones held a pidfd.
    assert!(proof["observed"].as_u64().unwrap() >= 150, "{proof}");
    // About 25 children are alive at once, plus the guard's own 7 or so.
    assert!(peak < 48, "the guard held {peak} descriptors");
}

fn an_observation_failure_while_running_does_not_stop_the_browser() {
    // 12 descriptors leave room for only a few pidfds: observing ~25 live
    // children fails with EMFILE while the browser runs.
    let (proof, status, _) = churn("emfile", 12);
    let failures = proof["nonFatalErrors"].as_array().unwrap();
    assert!(
        failures
            .iter()
            .any(|name| name.as_str().unwrap().ends_with("(errno 24)")),
        "{proof}"
    );
    // The browser still ran to its own normal end, and the drain completed.
    assert_eq!(status.code(), Some(0), "{proof}");
    assert!(normal(&proof), "{proof}");
}

fn errno_12(proof: &Value, field: &str) -> bool {
    proof[field]
        .as_array()
        .unwrap()
        .iter()
        .any(|name| name.as_str().unwrap().ends_with("(errno 12)"))
}

fn pruning_needs_no_free_descriptor() {
    let case = Case::new("prune");
    let mut command = case.command(case.receipt(), "churn");
    command.args(["150", "0.3", "0.012", "hold"]);
    at_start(&mut command, descriptor_limit(64, 64));
    let mut guard = case.spawn(command);
    guard.records.wait("root");
    // Once the guard holds many pidfds, take away every free descriptor.
    let deadline = Instant::now() + Duration::from_secs(15);
    while descriptor_count(guard.pid()) < 15 {
        assert!(
            Instant::now() < deadline,
            "the guard never held 15 descriptors"
        );
        pause(10);
    }
    let set_soft = |soft: u64| {
        let limit = libc::rlimit64 {
            rlim_cur: soft,
            rlim_max: 64,
        };
        // SAFETY: valid rlimit pointer; this test owns the guard process.
        let result = unsafe {
            libc::prlimit64(
                guard.pid(),
                libc::RLIMIT_NOFILE,
                &limit,
                std::ptr::null_mut(),
            )
        };
        assert_eq!(result, 0, "{}", io::Error::last_os_error());
    };
    set_soft(3);
    guard.records.wait("held");
    // Every child has been reaped: only the guard's own descriptors, the
    // browser's pidfd and its retained handle may stay open.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut held = descriptor_count(guard.pid());
    while held > 7 && Instant::now() < deadline {
        pause(50);
        held = descriptor_count(guard.pid());
    }
    set_soft(64);
    std::fs::write(case.path("release"), "").unwrap();
    let status = guard.wait(Duration::from_secs(20));
    let proof = case.proof();
    assert!(
        held <= 7,
        "{held} descriptors stayed open without a free one"
    );
    assert_eq!(status.code(), Some(0), "{proof}");
    assert!(normal(&proof), "{proof}");
}

fn transient_reap_and_wait_failures_do_not_stop_the_browser() {
    let case = Case::new("transient");
    let mut command = case.command(case.receipt(), "long");
    command
        .env("TAKOSUMI_GUARD_FAULT_REAP", "3")
        .env("TAKOSUMI_GUARD_FAULT_WAIT", "3");
    let status = case.spawn(command).wait(Duration::from_secs(20));
    let proof = case.proof();
    // The stand-in ran its full 3.5 s and ended on its own.
    assert_eq!(status.code(), Some(0), "{proof}");
    assert!(normal(&proof), "{proof}");
    assert!(errno_12(&proof, "nonFatalErrors"), "{proof}");
}

fn persistent_wait_failures_drain_after_the_bound() {
    let case = Case::new("persistent");
    let mut command = case.command(case.receipt(), "sleep");
    command.env("TAKOSUMI_GUARD_FAULT_WAIT", "always");
    let started = Instant::now();
    let mut guard = case.spawn(command);
    guard.records.wait("root");
    let status = guard.wait(Duration::from_secs(20));
    let elapsed = started.elapsed();
    let proof = case.proof();
    assert_eq!(status.code(), Some(1), "{proof}");
    assert!(
        proof["error"].as_str().unwrap().ends_with("(errno 12)"),
        "{proof}"
    );
    assert_eq!(proof["rootExit"], -libc::SIGTERM, "{proof}");
    assert_eq!(proof["complete"], true);
    // Eight failed ticks of 250 ms, then the 0.5 s natural phase.
    assert!(elapsed >= Duration::from_millis(1900), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
}

fn a_missing_browser_pidfd_does_not_stop_the_browser() {
    let case = Case::new("nopidfd");
    let mut command = case.command(case.receipt(), "normal");
    command.env("TAKOSUMI_GUARD_FAULT_CHILD_PIDFD", "always");
    let status = case.spawn(command).wait(Duration::from_secs(20));
    let proof = case.proof();
    assert_eq!(status.code(), Some(0), "{proof}");
    assert!(normal(&proof), "{proof}");
    assert!(errno_12(&proof, "nonFatalErrors"), "{proof}");
}

fn without_any_pidfd_the_drain_still_signals_the_browser() {
    // Neither the browser's own pidfd nor any observation: only its pid,
    // still unreaped, is left to signal.
    let case = Case::new("bypid");
    let mut command = case.command(case.receipt(), "sleep");
    command
        .env("TAKOSUMI_GUARD_FAULT_CHILD_PIDFD", "always")
        .env("TAKOSUMI_GUARD_FAULT_OBSERVE", "always");
    let mut guard = case.spawn(command);
    let root = guard.records.wait("root");
    guard.signal(libc::SIGTERM);
    let status = guard.wait(Duration::from_secs(20));
    let proof = case.proof();
    assert_eq!(status.code(), Some(1), "{proof}");
    assert_eq!(proof["rootExit"], -libc::SIGTERM, "{proof}");
    assert_eq!(proof["complete"], true, "{proof}");
    assert_eq!(proof["kernelNoChildren"], true);
    assert!(errno_12(&proof, "cleanupErrors"), "{proof}");
    assert!(ended(root));
}

fn an_inherited_sigchld_ignore_keeps_the_browser_exit_status() {
    let case = Case::new("sigchld");
    let mut command = case.command(case.receipt(), "normal");
    at_start(&mut command, || unsafe {
        if libc::signal(libc::SIGCHLD, libc::SIG_IGN) == libc::SIG_ERR {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    });
    let status = case.spawn(command).wait(Duration::from_secs(20));
    let proof = case.proof();
    // Auto-reaping would have lost the exit status (rootExit null).
    assert_eq!(status.code(), Some(0), "{proof}");
    assert_eq!(proof["rootExit"], 0, "{proof}");
    assert!(clean(&proof), "{proof}");
}

fn the_browser_gets_only_stdio_and_the_original_descriptor_limit() {
    let case = Case::new("fds");
    let leaked = std::fs::File::create(case.path("leaked")).unwrap();
    let source = leaked.as_raw_fd();
    let mut command = case.command(case.receipt(), "fds");
    // An inheritable descriptor (dup2 clears close-on-exec) the App leaked.
    at_start(&mut command, move || {
        check(unsafe { libc::dup2(source, 37) })
    });
    at_start(&mut command, descriptor_limit(64, 128));
    let mut guard = case.spawn(command);
    guard.records.wait("fds");
    let held = std::fs::read_link(format!("/proc/{}/fd/37", guard.pid()));
    let limits = std::fs::read_to_string(format!("/proc/{}/limits", guard.pid())).unwrap();
    std::fs::write(case.path("release"), "").unwrap();
    let status = guard.wait(Duration::from_secs(20));
    let proof = case.proof();
    assert_eq!(status.code(), Some(0), "{proof}");
    assert!(clean(&proof), "{proof}");
    assert_eq!(
        held.unwrap(),
        case.path("leaked").canonicalize().unwrap(),
        "the guard got no leak"
    );
    let raised = limits
        .lines()
        .find(|line| line.starts_with("Max open files"))
        .unwrap();
    assert_eq!(
        raised.split_whitespace().collect::<Vec<_>>()[3..5],
        ["128", "128"]
    );
    let seen: Value =
        serde_json::from_slice(&std::fs::read(case.path("fds.json")).unwrap()).unwrap();
    assert_eq!(seen, json!([[], 64, 128]), "the browser saw {seen}");
}

fn the_browser_keeps_the_control_pipe_on_stdin_and_stdout() {
    let case = Case::new("pipe");
    let mut command = case.command(case.receipt(), "echo");
    command
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut guard = Guarded {
        child,
        records: Records::default(),
    };
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
    assert!(clean(&proof), "{proof}");
}

fn a_pending_stop_starts_no_browser() {
    let case = Case::new("pending");
    let mut command = case.command(case.receipt(), "marker");
    // A SIGTERM already pending (blocked) when the guard starts.
    at_start(&mut command, || unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        check(libc::sigprocmask(
            libc::SIG_BLOCK,
            &set,
            std::ptr::null_mut(),
        ))?;
        check(libc::kill(libc::getpid(), libc::SIGTERM))
    });
    let status = case.spawn(command).wait(Duration::from_secs(20));
    let proof = case.proof();
    assert_eq!(status.code(), Some(1), "{proof}");
    assert_eq!(proof["rootExit"], Value::Null);
    assert_eq!(proof["complete"], false);
    assert_eq!(proof["error"], "StopRequestedBeforeStart");
    assert_eq!(proof["kernelNoChildren"], true);
    pause(300);
    assert!(!case.path("ran").exists(), "the browser started");
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
    assert_eq!(proof["error"], "NotFound (errno 2)");
    assert_eq!(proof["kernelNoChildren"], true);
}

fn ownership_requires_a_stable_chain_to_this_exact_owner() {
    // SAFETY: getpid has no preconditions.
    let me = unsafe { libc::getpid() };
    let my_start = start_time(me).unwrap().unwrap();
    let mine = sleeper(false);
    let sibling = sleeper(false);
    let mine_start = start_time(mine.pid()).unwrap().unwrap();
    assert_eq!(is_owned(mine.pid(), me, my_start), Ok(true));
    // The same pid with another start time is a different (reused) owner.
    assert_eq!(is_owned(mine.pid(), me, my_start + 1), Ok(false));
    // A process from another ancestry, as a stale snapshot might name it.
    assert_eq!(is_owned(sibling.pid(), mine.pid(), mine_start), Ok(false));
}

/// A shell child of this test with one `sleep` grandchild, both cleaned up
/// however the test ends (the grandchild through a pidfd, first).
fn shell_with_grandchild() -> (KillOnDrop, (i32, u64), Reaped, (i32, u64)) {
    let mut shell = Command::new("sh");
    shell
        .args(["-c", "sleep 30 & echo $!; wait"])
        .stdout(Stdio::piped());
    let mut shell = Reaped(shell.spawn().unwrap());
    let mut line = String::new();
    BufReader::new(shell.0.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let grandchild = line.trim().parse::<i32>().unwrap();
    // The shell is alive and has not reaped it, so this names the grandchild.
    let killer = KillOnDrop(pidfd_open(grandchild).unwrap());
    let records = [shell.pid(), grandchild].map(|pid| (pid, start_time(pid).unwrap().unwrap()));
    (killer, records[1], shell, records[0])
}

fn the_tracker_retains_every_descendant_with_its_start_time() {
    // SAFETY: getpid has no preconditions.
    let me = unsafe { libc::getpid() };
    let (_grandchild_killer, grandchild, _shell_reaped, shell) = shell_with_grandchild();
    let mut tracker = Tracker::default();
    tracker.observe(me).unwrap();
    let retained: Vec<(i32, u64)> = tracker
        .handles
        .iter()
        .map(|(pid, (start, _, _))| (*pid, *start))
        .collect();
    let mut expected = vec![shell, grandchild];
    expected.sort_unstable();
    assert_eq!(
        retained, expected,
        "only the shell and its child are below the runner"
    );
    assert_eq!(tracker.observed, 2);
}

fn the_tracker_releases_handles_of_ended_processes() {
    // SAFETY: getpid has no preconditions.
    let me = unsafe { libc::getpid() };
    let (grandchild_killer, grandchild, _shell_reaped, shell) = shell_with_grandchild();
    let mut tracker = Tracker::default();
    tracker.observe(me).unwrap();
    assert_eq!(tracker.handles.len(), 2);
    // End the grandchild; the shell's `wait` reaps it, then the shell exits
    // and stays an unreaped zombie of this test.
    drop(grandchild_killer);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ended(grandchild) {
        assert!(Instant::now() < deadline, "the grandchild was not reaped");
        pause(10);
    }
    tracker.observe(me).unwrap();
    // The reaped grandchild's pidfd is released; the zombie shell keeps its.
    let retained: Vec<i32> = tracker.handles.keys().copied().collect();
    assert_eq!(retained, [shell.0]);
    assert_eq!(tracker.observed, 2, "an ended process was counted again");
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

// STOP_OBSERVATION_CONTROL_BEGIN
fn stop_observation_is_bounded_and_uses_only_existing_results() {
    let mut observation = StopObservation::default();
    observation.reaped(6, false, 0);
    assert!(observation.json()["records"].as_array().unwrap().is_empty());
    observation.begin();
    observation.signal(
        7,
        Some(42),
        true,
        "retained_pidfd",
        libc::SIGTERM,
        &Ok(()),
        None,
    );
    observation.signal(
        8,
        None,
        true,
        "direct_pid",
        libc::SIGKILL,
        &Err(io::Error::from_raw_os_error(libc::ESRCH)),
        None,
    );
    observation.reaped(7, true, 0);
    let proof = observation.json();
    let records = proof["records"].as_array().unwrap();
    assert_eq!(records[0]["startTick"], 42);
    assert_eq!(records[0]["result"], "ok");
    assert_eq!(records[1]["startTick"], serde_json::Value::Null);
    assert_eq!(records[1]["result"], "esrch");
    assert_eq!(records[1]["errno"], libc::ESRCH);
    assert_eq!(records[2]["startTick"], serde_json::Value::Null);
    assert_eq!(records[2]["waitStatus"], 0);
    for _ in 0..STOP_OBSERVATION_CAP {
        observation.reaped(9, false, 0);
    }
    let proof = observation.json();
    assert_eq!(
        proof["records"].as_array().unwrap().len(),
        STOP_OBSERVATION_CAP
    );
    assert_eq!(proof["truncated"], true);
    assert!(serde_json::to_vec(&proof).unwrap().len() < 16 * 1024);
}
// STOP_OBSERVATION_CONTROL_END

// TERMINAL_SIGNAL_CONTROL_BEGIN
const TERMINAL_LOCAL_CHILD: &str = r#"
import os,signal,sys
signal.signal(signal.SIGTERM, lambda *_: os._exit(0))
if sys.argv[1].startswith("comm-"):
    import ctypes
    name = b"adb" if sys.argv[1] == "comm-adb" else b"private-label"
    if ctypes.CDLL(None).prctl(15, name, 0, 0, 0) != 0: os._exit(93)
print("ready", flush=True)
sys.stdin.readline()
os._exit(0 if sys.argv[1] == "exit" else 2)
"#;

// These pidfds name only the child created here. poll never reaps it.
fn terminal_local_wait(fd: &OwnedFd, millis: i32) -> bool {
    let mut item = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN | libc::POLLRDNORM,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut item, 1, millis) };
    result == 1
        && item.revents & libc::POLLIN != 0
        && item.revents & !(libc::POLLIN | libc::POLLRDNORM | libc::POLLHUP) == 0
}

struct TerminalLocalChild {
    child: Child,
    fd: OwnedFd,
    reaped: bool,
}

// The temporary owner is established immediately after spawn, before fd-open
// or any injected failure. A successful transfer takes the Child exactly once.
static TERMINAL_CTOR_CLEANUPS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
struct TerminalSpawnOwner(Option<Child>);

impl Drop for TerminalSpawnOwner {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            // This is still our unreaped direct child: no Run or other waiter
            // has received it, so its numeric PID cannot have been recycled.
            if child.kill().is_err() || child.wait().is_err() {
                let _ = writeln!(io::stderr(), "OWN_LOCAL_CHILD_CTOR_CLEANUP incomplete");
                std::process::abort(); // Never continue catch_unwind with unknown cleanup.
            }
            let mut status = 0;
            let result = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG | libc::__WALL) };
            let empty =
                result == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD);
            if !empty {
                let _ = writeln!(io::stderr(), "OWN_LOCAL_CHILD_CTOR_CLEANUP no_echild");
                std::process::abort();
            }
            TERMINAL_CTOR_CLEANUPS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = writeln!(
                io::stdout(),
                "OWN_LOCAL_CHILD_CTOR_CLEANUP reaped=true echild=true"
            );
        }
    }
}

impl TerminalLocalChild {
    fn new(mode: &str) -> Self {
        Self::try_new_with_fd(mode, pidfd_open).unwrap()
    }

    fn try_new_with_fd(
        mode: &str,
        open: impl FnOnce(i32) -> io::Result<OwnedFd>,
    ) -> io::Result<Self> {
        let mut pending = TerminalSpawnOwner(Some(
            Command::new("/usr/bin/python3")
                .args(["-I", "-S", "-c", TERMINAL_LOCAL_CHILD, mode])
                .env_clear()
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()?,
        ));
        let fd = open(pending.0.as_ref().unwrap().id() as i32)?;
        let mut owned = Self {
            child: pending.0.take().unwrap(),
            fd,
            reaped: false,
        };
        let mut line = String::new();
        BufReader::new(owned.child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line, "ready\n");
        Ok(owned)
    }
}

impl Drop for TerminalLocalChild {
    fn drop(&mut self) {
        if !self.reaped {
            if !terminal_local_wait(&self.fd, 0) {
                let _ = pidfd_signal(&self.fd, libc::SIGKILL);
            }
            if terminal_local_wait(&self.fd, 3_000) {
                self.reaped = self.child.wait().is_ok();
            }
            eprintln!("OWN_LOCAL_CHILD_CLEANUP drop_reaped={}", self.reaped);
        }
    }
}

fn terminal_local_run(child: &TerminalLocalChild) -> Run {
    let mut run = Run {
        me: std::process::id() as i32,
        tracker: Tracker::default(),
        child: Some(child.child.id() as i32),
        child_fd: Some(child.fd.try_clone().unwrap()),
        root_exit: None,
        forced: BTreeSet::new(),
        cleanup_errors: BTreeSet::new(),
        non_fatal: BTreeSet::new(),
        stop_observation: StopObservation::default(),
    };
    // The existing ownership observer retains this live child's genuine pidfd/start tick.
    run.safe_observe();
    assert!(run.tracker.handles.contains_key(&(child.child.id() as i32)));
    run.stop_observation.begin();
    run
}

fn terminal_local_finish(run: &mut Run, child: &mut TerminalLocalChild, label: &str) {
    assert!(
        terminal_local_wait(&child.fd, 3_000),
        "owned child did not reach terminal readiness"
    );
    let empty = run.reap().unwrap();
    child.reaped = empty;
    println!(
        "OWN_LOCAL_CHILD_CLEANUP case={label} echild={empty} root_exit={:?}",
        run.root_exit
    );
    assert!(empty, "owned local child was not reaped to ECHILD");
    assert_eq!(run.root_exit, Some(0));
    assert!(run.cleanup_errors.is_empty());
}

fn terminal_signal_owned_terminal_unreaped_child_is_not_forced() {
    let mut child = TerminalLocalChild::new("exit");
    let mut run = terminal_local_run(&child);
    let pid = child.child.id() as i32;
    let start = run.tracker.handles[&pid].0;
    child
        .child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"exit\n")
        .unwrap();
    assert!(terminal_local_wait(&run.tracker.handles[&pid].1, 3_000));
    // No wait occurred: the same flags=0 retained pidfd is already terminal.
    run.send(libc::SIGTERM);
    assert_eq!(
        run.tracker.handles[&pid].0, start,
        "terminal identity must remain retained until reap"
    );
    let proof = run.stop_observation.json();
    let signals = proof["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == "signal")
        .count();
    let ok = proof["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == "signal" && r["result"] == "ok")
        .count();
    terminal_local_finish(&mut run, &mut child, "terminal-before-send");
    println!("OWN_TERMINAL_CONTRAST terminal_before_send=true forced={} signals={signals} signal_ok={ok}", run.forced.len());
    assert!(
        run.forced.is_empty(),
        "already terminal owned child must not count as forced"
    );
    assert_eq!(
        signals, 0,
        "retained and direct-pidfd paths must both skip known terminal children"
    );
}

fn terminal_signal_owned_live_handler_exit_zero_stays_forced() {
    let mut child = TerminalLocalChild::new("live");
    let mut run = terminal_local_run(&child);
    let pid = child.child.id() as i32;
    assert!(!terminal_local_wait(&run.tracker.handles[&pid].1, 0));
    run.send(libc::SIGTERM);
    terminal_local_finish(&mut run, &mut child, "live-handler-exit-zero");
    println!(
        "OWN_TERMINAL_CONTRAST live_before_send=true forced={} root_exit_zero=true",
        run.forced.len()
    );
    assert!(
        run.forced.contains(&pid),
        "live signal Ok must remain forced after handler exit 0"
    );
}
// TERMINAL_SIGNAL_CONTROL_END

// TERMINAL_SIGNAL_EXTRA_CONTROL_BEGIN
fn terminal_signal_readiness_unknown_keeps_eligibility() {
    for events in [
        libc::POLLIN,
        libc::POLLIN | libc::POLLRDNORM,
        libc::POLLIN | libc::POLLRDNORM | libc::POLLHUP,
    ] {
        assert!(pidfd_terminal_ready(1, events));
    }
    for (polled, events) in [
        (-1, libc::POLLIN),
        (0, libc::POLLIN),
        (2, libc::POLLIN),
        (1, 0),
        (1, libc::POLLHUP),
        (1, libc::POLLRDNORM),
        (1, libc::POLLIN | libc::POLLERR),
        (1, libc::POLLIN | libc::POLLNVAL),
        (1, libc::POLLIN | libc::POLLPRI),
    ] {
        assert!(
            !pidfd_terminal_ready(polled, events),
            "unknown readiness must remain signal-eligible"
        );
    }
}

fn terminal_signal_owned_no_direct_fd_preserves_pid_fallback() {
    struct Restore(Option<OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(value) = &self.0 {
                std::env::set_var("TAKOSUMI_GUARD_FAULT_CHILD_PIDFD", value);
            } else {
                std::env::remove_var("TAKOSUMI_GUARD_FAULT_CHILD_PIDFD");
            }
        }
    }
    let mut child = TerminalLocalChild::new("live");
    let mut run = terminal_local_run(&child);
    let pid = child.child.id() as i32;
    assert!(!terminal_local_wait(&child.fd, 0));
    run.child_fd = None;
    let restore = Restore(std::env::var_os("TAKOSUMI_GUARD_FAULT_CHILD_PIDFD"));
    std::env::set_var("TAKOSUMI_GUARD_FAULT_CHILD_PIDFD", "always");
    run.send(libc::SIGTERM);
    drop(restore);
    let proof = run.stop_observation.json();
    assert!(proof["records"]
        .as_array()
        .unwrap()
        .iter()
        .any(|record| record["kind"] == "signal" && record["target"] == "direct_pid"));
    terminal_local_finish(&mut run, &mut child, "owned-no-direct-fd");
    assert!(run.forced.contains(&pid));
}
// TERMINAL_SIGNAL_EXTRA_CONTROL_END

// TERMINAL_SIGNAL_V2_CONTROL_BEGIN
fn terminal_signal_owned_fd_open_failure_cleans_partial_child() {
    use std::sync::atomic::Ordering;
    let before = TERMINAL_CTOR_CLEANUPS.load(Ordering::SeqCst);
    for errno in [libc::EMFILE, libc::ENOMEM] {
        let opened = std::cell::Cell::new(false);
        let result = TerminalLocalChild::try_new_with_fd("live", |_pid| {
            opened.set(true);
            Err(io::Error::from_raw_os_error(errno))
        });
        let original = match result {
            Err(error) => error,
            Ok(_) => panic!("injected fd-open failure unexpectedly succeeded"),
        };
        assert!(opened.get());
        assert_eq!(original.raw_os_error(), Some(errno));
        println!("OWN_LOCAL_CHILD_FD_OPEN_ERROR original_errno={errno} preserved=true");
    }
    let original_panic = 0x5411_cafe_u64;
    let caught = std::panic::catch_unwind(|| {
        let _ = TerminalLocalChild::try_new_with_fd("live", |_pid| -> io::Result<OwnedFd> {
            std::panic::panic_any(original_panic)
        });
    });
    let payload = caught
        .err()
        .expect("the original injected panic must propagate");
    assert_eq!(payload.downcast_ref::<u64>(), Some(&original_panic));
    assert_eq!(TERMINAL_CTOR_CLEANUPS.load(Ordering::SeqCst), before + 3);
    println!("OWN_LOCAL_CHILD_FD_OPEN_PANIC original_payload_preserved=true ctor_cleanups=3");
}
// TERMINAL_SIGNAL_V2_CONTROL_END

// READINESS_OBSERVATION_CONTROL_BEGIN
fn guard_readiness_observation_preserves_errno_optional_and_budget() {
    struct RestoreErrno(i32);
    impl Drop for RestoreErrno {
        fn drop(&mut self) {
            unsafe {
                *libc::__errno_location() = self.0;
            }
        }
    }
    let _restore = RestoreErrno(unsafe { *libc::__errno_location() });
    let polls = std::cell::Cell::new(0);
    let failed = pidfd_readiness_with(-1, |entry| {
        polls.set(polls.get() + 1);
        assert_eq!(entry.events, libc::POLLIN | libc::POLLRDNORM);
        assert_eq!(entry.revents, 0);
        entry.revents = libc::POLLERR;
        unsafe {
            *libc::__errno_location() = libc::EINTR;
        }
        -1
    });
    assert_eq!(polls.get(), 1);
    assert_eq!(failed.polled, -1);
    assert_eq!(failed.revents, libc::POLLERR);
    assert!(!failed.terminal);
    assert_eq!(failed.poll_errno, Some(libc::EINTR));
    // Later activity poisons TLS errno; both captured errors must remain distinct.
    unsafe {
        *libc::__errno_location() = libc::EINVAL;
    }
    let sent = Err(io::Error::from_raw_os_error(libc::EPERM));
    unsafe {
        *libc::__errno_location() = libc::ENOMEM;
    }
    let mut observation = StopObservation::default();
    observation.begin();
    observation.signal(
        7,
        Some(42),
        false,
        "retained_pidfd",
        libc::SIGTERM,
        &sent,
        Some(failed),
    );
    let proof = observation.json();
    assert_eq!(
        proof["readinessFields"],
        serde_json::json!(["polled", "revents", "terminal", "pollErrno"])
    );
    assert_eq!(
        proof["records"][0]["readiness"],
        serde_json::json!([-1, libc::POLLERR, false, libc::EINTR])
    );
    assert_eq!(proof["records"][0]["errno"], libc::EPERM);

    let quiet = pidfd_readiness_with(-1, |_entry| {
        unsafe {
            *libc::__errno_location() = libc::EIO;
        }
        0
    });
    assert!(!quiet.terminal);
    assert_eq!(quiet.poll_errno, None); // A successful poll never borrows stale errno.
    let positive = pidfd_readiness_with(-1, |entry| {
        entry.revents = libc::POLLIN;
        1
    });
    assert!(positive.terminal);
    assert_eq!(positive.poll_errno, None);
    assert_eq!(observation.used, 1); // Capturing a skipped result creates no signal record.

    let callbacks = std::cell::Cell::new(0);
    let no_signal = signal_readiness(0, Some(&()), |_| {
        callbacks.set(callbacks.get() + 1);
        quiet
    });
    let no_fd = signal_readiness(libc::SIGTERM, None::<&()>, |_| {
        callbacks.set(callbacks.get() + 1);
        quiet
    });
    assert!(no_signal.is_none() && no_fd.is_none());
    assert_eq!(callbacks.get(), 0);
    let once = signal_readiness(libc::SIGTERM, Some(&()), |_| {
        callbacks.set(callbacks.get() + 1);
        quiet
    });
    assert_eq!(callbacks.get(), 1);
    assert_eq!(once.unwrap().polled, 0);
    observation.signal(8, None, true, "direct_pid", 0, &Ok(()), no_signal);
    observation.reaped(7, false, 0);
    let proof = observation.json();
    assert_eq!(proof["records"][1]["readiness"], serde_json::Value::Null);
    assert_eq!(proof["records"][2]["readiness"], serde_json::Value::Null);
    assert_eq!(proof["records"].as_array().unwrap().len(), 3);

    let worst = PidfdReadiness {
        polled: i32::MIN,
        revents: i16::MIN,
        terminal: false,
        poll_errno: Some(i32::MIN),
    };
    let failed_signal = Err(io::Error::from_raw_os_error(i32::MIN));
    let mut full = StopObservation::default();
    full.begin();
    for _ in 0..STOP_OBSERVATION_CAP {
        full.signal(
            i32::MAX,
            Some(u64::MAX),
            false,
            "retained_pidfd",
            libc::SIGUSR1,
            &failed_signal,
            Some(worst),
        );
    }
    full.reaped(1, false, 0);
    let proof = full.json();
    assert_eq!(
        proof["records"].as_array().unwrap().len(),
        STOP_OBSERVATION_CAP
    );
    assert_eq!(proof["truncated"], true);
    let bytes = serde_json::to_vec(&proof).unwrap();
    assert!(bytes.len() < 16 * 1024);
    println!("READINESS_OBSERVATION pure=true cap={} bytes={} poll_errno_preserved=true signal_errno_separate=true", STOP_OBSERVATION_CAP, bytes.len());
    assert_eq!(polls.get(), 1);
    assert_eq!(callbacks.get(), 1); // Serialization never repeats either poll path.
}
// READINESS_OBSERVATION_CONTROL_END

// COMM_CLASS_OBSERVATION_CONTROL_BEGIN
fn guard_comm_class_projection_is_private_optional_and_bounded() {
    let mut fields = vec!["0"; 20];
    fields[0] = "S";
    fields[1] = "17";
    fields[19] = "42";
    let fields = fields.join(" ");
    for (name, expected) in [
        ("adb", CommClass::Adb),
        ("emulator", CommClass::Emulator),
        ("qemu-system-x86", CommClass::Qemu),
        ("qemu-system-aar", CommClass::Qemu),
        ("crashpad_handle", CommClass::Crashpad),
        ("private label", CommClass::Other),
        ("private) S 999 ", CommClass::Other),
        ("", CommClass::Unknown),
    ] {
        let text = format!("9 ({name}) {fields}");
        assert_eq!(parse_stat(&text), Some((42, 17)));
        assert_eq!(comm_class(&text), expected);
    }
    assert_eq!(comm_class("not a stat; private label"), CommClass::Unknown);
    assert_eq!(
        comm_class("9 (unterminated private label"),
        CommClass::Unknown
    );

    let mut observation = StopObservation::default();
    observation.begin();
    observation.signal_with_comm(
        9,
        Some(42),
        false,
        "retained_pidfd",
        libc::SIGTERM,
        &Ok(()),
        None,
        Some(CommClass::Adb),
    );
    observation.signal(9, None, true, "direct_pidfd", libc::SIGTERM, &Ok(()), None);
    observation.reaped(9, false, 0);
    let proof = observation.json();
    assert_eq!(proof["commClasses"][CommClass::Adb as usize], "adb_name");
    assert_eq!(proof["records"][0]["commClass"], CommClass::Adb as u8);
    assert_eq!(proof["records"][1]["commClass"], Value::Null);
    assert_eq!(proof["records"][2]["commClass"], Value::Null);
    assert_eq!(proof["records"][2]["startTick"], Value::Null);
    assert!(!serde_json::to_string(&proof).unwrap().contains("private"));

    let mut full = StopObservation::default();
    full.begin();
    let worst = PidfdReadiness {
        polled: i32::MIN,
        revents: i16::MIN,
        terminal: false,
        poll_errno: Some(i32::MIN),
    };
    let error = Err(io::Error::from_raw_os_error(i32::MIN));
    for _ in 0..STOP_OBSERVATION_CAP {
        full.signal_with_comm(
            i32::MAX,
            Some(u64::MAX),
            false,
            "retained_pidfd",
            libc::SIGUSR1,
            &error,
            Some(worst),
            None,
        );
    }
    full.reaped(9, false, 0);
    let value = full.json();
    assert_eq!(value["records"].as_array().unwrap().len(), 64);
    assert_eq!(value["truncated"], true);
    let bytes = serde_json::to_vec(&value).unwrap().len();
    assert!(bytes < 16 * 1024, "existing observer byte budget: {bytes}");
    println!("COMM_CLASS pure=true cap=64 bytes={bytes} raw_name_saved=false");
}

fn guard_comm_class_owned_names_do_not_change_forced_cleanup() {
    for (mode, expected) in [
        ("comm-adb", CommClass::Adb),
        ("comm-private", CommClass::Other),
    ] {
        let mut child = TerminalLocalChild::new(mode);
        let mut run = terminal_local_run(&child);
        let pid = child.child.id() as i32;
        let start = run.tracker.handles[&pid].0;
        let captured = run.tracker.handles[&pid].2;
        run.send(libc::SIGTERM);
        // Reap before any payload assertions, including a failed assertion.
        terminal_local_finish(&mut run, &mut child, "comm-class-owned-child");
        assert_eq!(captured, expected);
        let proof = run.stop_observation.json();
        let records = proof["records"].as_array().unwrap();
        let retained: Vec<_> = records
            .iter()
            .filter(|r| r["target"] == "retained_pidfd")
            .collect();
        assert!(!retained.is_empty());
        for record in retained {
            assert_eq!(record["pid"], pid);
            assert_eq!(record["startTick"], start);
            assert_eq!(record["commClass"], expected as u8);
        }
        for record in records
            .iter()
            .filter(|r| r["kind"] == "reap" || r["target"] == "direct_pidfd")
        {
            assert_eq!(record["commClass"], Value::Null);
        }
        assert_eq!(run.forced.len(), 1, "a name never exempts a live child");
        assert!(!serde_json::to_string(&proof)
            .unwrap()
            .contains("private-label"));
        println!("COMM_CLASS owned=true name_spoofable=true forced=1 reaped=true echild=true");
    }
}
// COMM_CLASS_OBSERVATION_CONTROL_END
