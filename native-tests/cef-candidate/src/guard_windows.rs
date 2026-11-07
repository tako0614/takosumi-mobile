//! Product supervisor for one per-profile CEF browser process (Windows).
//!
//! `takosumi-browser-guard <absolute new receipt> <program> [args...]`
//!
//! The same command line, receipt and exit codes as the Linux guard
//! (`guard_main.rs`), built on a Job Object instead of pidfds and the child
//! subreaper. The App starts this guard inside its own kill-on-close Job
//! Object (see `tauri_plugin_app_screen::windows_job`), so when the App ends,
//! however it ends, its job ends this guard and the browser: Windows has no
//! parent-death signal. The guard refuses to run outside any job.
//!
//! The guard creates a kill-on-close Job Object of its own (no breakaway) and
//! starts the browser in it before it runs any code (created suspended,
//! assigned, resumed), passing its own standard input, output and error: the
//! App's control pipe, reply pipe and log. Every process the browser starts
//! is then in the guard's job. The browser is a GUI-subsystem program, so
//! neither it nor its CEF helpers (the same executable) have a console or a
//! console host; `CREATE_NO_WINDOW` stays, and would only matter for a
//! console program. The App starts the guard itself detached from any
//! console, so it has no console host either. It waits for the
//! browser to exit or for a stop request: the App sets a named event it
//! created and passed in `TAKOSUMI_BROWSER_GUARD_STOP_EVENT` (removed from
//! the browser's environment). There is no lifetime cap and no polling while
//! it waits.
//!
//! The drain then always runs: up to 0.5 s for the browser and every process
//! in the job to end by themselves, then `TerminateJobObject` (each process
//! still in the job ends with the exit code `FORCED_EXIT`), then up to 2 s for
//! the job to empty. Forced cleanup counts the processes that were in the job
//! before the termination and ended with `FORCED_EXIT` (or still run after
//! it), plus any that entered the job after that list was read; one whose end
//! cannot be attributed is a cleanup error (the receipt is then not normal),
//! never a guess. Each listed process is classified (`classify`) from whether
//! its handle is signaled, read first, and then its `GetExitCodeProcess` code,
//! which Windows sets before the job drops the process and before it signals
//! the handle: a zero wait on the handle alone read a process that had just
//! ended by itself as still running (run 42343580278 counted 56 such as
//! forced). Since 6302b33 the count is taken that way. Between the list and
//! the termination the guard also identifies each listed process (below), so
//! the effective natural window is 0.5 s plus that time (`identifyMs`). The
//! completion proof is the job's `ActiveProcesses == 0`
//! (`JobObjectBasicAccountingInformation`) after the browser exited.
//!
//! Receipt, written once after the drain, with the Linux guard's fields:
//! `{rootExit, complete, remaining, kernelNoChildren, observed, forcedCleanup,
//! error, cleanupErrors, nonFatalErrors}`. Here `kernelNoChildren` keeps its
//! key with the meaning "the kernel reports the job empty", `remaining` lists
//! the process ids still in the job (`JobObjectBasicProcessIdList`) and
//! `observed` counts every process ever in it (`TotalProcesses`, the browser
//! included). `rootExit` is the browser's exit code (NTSTATUS crash codes are
//! negative). The guard exits 0 only when the drain proved the job empty,
//! the browser exited 0 and nothing failed; as on Linux a forced cleanup
//! alone does not change the exit status, and the App separately requires
//! `forcedCleanup == 0`. A refusal before the receipt exists exits 2 and
//! starts nothing.
//!
//! Diagnostics (`guard_windows_identity.rs`): when the drain reaches the
//! termination, the guard reads each listed process's image file name and
//! Chromium process type just before it, and afterwards writes one
//! `takosumi-browser-guard: forced-cleanup {...}` line to its standard error
//! naming the processes it counted as forced (none, if all ended by
//! themselves). The bounds, the receipt's fields and the exit status are
//! unchanged by them.
//!
//! Not covered: a process that the browser has created outside its tree
//! through a system service (COM, WMI, the task scheduler) is not in the job,
//! as on Linux a process started by a session manager is not a descendant.

#[path = "guard_windows_identity.rs"]
mod identity;
#[path = "process_type.rs"]
mod process_type;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::path::Path;
use std::process::{Child, Command, ExitCode};
use std::time::{Duration, Instant};
use tauri_plugin_app_screen::windows_job::{
    in_job, open_process, spawn_in, wait_any, Event, Job, CREATE_NO_WINDOW, STOP_EVENT_ENV,
};

#[link(name = "kernel32")]
extern "system" {
    fn GetExitCodeProcess(process: *mut std::ffi::c_void, code: *mut u32) -> i32;
}

/// How a listed process ended, for the forced count.
#[derive(Debug, PartialEq, Eq)]
enum Ended {
    /// Ended by the termination, or still running after it.
    Forced,
    /// Ended by itself, with any other exit code.
    ByItself,
    /// Its exit code could not be read.
    Unknown,
}

/// The forced count's rule, from a held process's exit code (None: the read
/// failed) and whether its handle was signaled before the code was read.
/// `FORCED_EXIT` is the termination's. `STILL_ACTIVE` is what Windows reports
/// while a process runs, but also a code a program can return: it is the
/// latter only once the handle is signaled. Any other code is final (Windows
/// sets it before the handle is signaled), so the process ended by itself.
fn classify(code: Option<u32>, signaled: bool) -> Ended {
    match code {
        None => Ended::Unknown,
        Some(FORCED_EXIT) => Ended::Forced,
        Some(STILL_ACTIVE) if !signaled => Ended::Forced,
        Some(_) => Ended::ByItself,
    }
}

fn exit_code_now(handle: &OwnedHandle) -> io::Result<u32> {
    let mut code = 0u32;
    // SAFETY: a live process handle and a valid result pointer.
    if unsafe { GetExitCodeProcess(handle.as_raw_handle().cast(), &mut code) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(code)
}

fn signaled(handle: &OwnedHandle, within: Duration) -> io::Result<bool> {
    Ok(wait_any(&[handle.as_raw_handle()], Some(within))?.is_some())
}

/// A held process's state for `classify`: whether its handle is signaled,
/// read first (so a code read afterwards is never older than it), then its
/// exit code. A `STILL_ACTIVE` code with the handle unsignaled while the job
/// was already seen empty is the moment just before Windows signals an ended
/// process: its handle is then given up to `grace` and both are read again.
fn exit_read(handle: &OwnedHandle, grace: Duration) -> io::Result<(u32, bool)> {
    let first = signaled(handle, Duration::ZERO)?;
    let code = exit_code_now(handle)?;
    if code == STILL_ACTIVE && !first && !grace.is_zero() {
        let later = signaled(handle, grace)?;
        return Ok((exit_code_now(handle)?, later));
    }
    Ok((code, first))
}

/// Natural end, then the job is terminated and may take this long to empty.
const NATURAL: Duration = Duration::from_millis(500);
const FORCED: Duration = Duration::from_secs(2);
/// While draining, re-check the job at least this often.
const DRAIN_TICK: Duration = Duration::from_millis(20);
/// After a failed wait, retry after this pause; this many consecutive
/// failures (about 2 s) make the guard give up waiting and drain.
const FAULT_PAUSE: Duration = Duration::from_millis(250);
const MAX_RUNNING_FAULTS: u32 = 8;
/// The exit code of processes the drain terminates: an NTSTATUS-shaped
/// customer code (`0xE...`) no program returns by accident, unlike 1. In the
/// receipt it appears as the negative `rootExit` -536841118.
const FORCED_EXIT: u32 = 0xE000_7462;
/// `OpenProcess` of an id no process has any more.
const ERROR_INVALID_PARAMETER: i32 = 87;
/// `GetExitCodeProcess` while a process runs (also a possible exit code).
const STILL_ACTIVE: u32 = 259;
/// The longest the count gives one `STILL_ACTIVE`, unsignaled process in an
/// empty job to be signaled; it also never runs past the end of the drain's
/// existing 2 s forced budget (`count_forced`).
const SIGNAL_GRACE: Duration = Duration::from_millis(50);
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

/// Called by `guard_entry.rs` on Windows.
pub(super) fn main() -> ExitCode {
    // The self-test build runs its tests, or acts as a stand-in browser,
    // unless it is asked to act as the guard; the release binary has no such
    // switch.
    #[cfg(test)]
    if let Some(code) = tests::dispatch() {
        return code;
    }
    let args: Vec<OsString> = std::env::args_os().collect();
    match prepare(&args) {
        Ok(prepared) => supervise(prepared, &args[2], &args[3..]),
        Err((code, message)) => {
            eprintln!("takosumi-browser-guard: {message}");
            ExitCode::from(code)
        }
    }
}

/// Self-test fault injection (`TAKOSUMI_GUARD_FAULT_<POINT>` set in the guard
/// under test), compiled into the self-test binary only; in a release build
/// every call is a constant `false`.
#[cfg(test)]
fn injected(point: &str) -> bool {
    std::env::var_os(format!("TAKOSUMI_GUARD_FAULT_{point}")).is_some()
}
#[cfg(not(test))]
#[inline(always)]
fn injected(_point: &str) -> bool {
    false
}

/// A short, stable error name for the receipt; never a path or argv.
fn fault(error: &io::Error) -> String {
    match error.raw_os_error() {
        Some(code) => format!("{:?} (Windows error {code})", error.kind()),
        None => format!("{:?}", error.kind()),
    }
}

struct Prepared {
    receipt: File,
    stop: Event,
    job: Job,
}

fn prepare(args: &[OsString]) -> Result<Prepared, (u8, String)> {
    let refuse = |message: &str| Err((2, message.to_string()));
    if args.len() < 3 {
        return refuse("usage: takosumi-browser-guard <absolute new receipt> <program> [args...]");
    }
    let receipt = Path::new(&args[1]);
    // Nothing may exist at the path, not even a dangling link.
    let vacant =
        matches!(receipt.symlink_metadata(), Err(error) if error.kind() == io::ErrorKind::NotFound);
    if !receipt.is_absolute() || !vacant {
        return refuse("The receipt path must be absolute and must not exist.");
    }
    // The App's job is this guard's only link to the App's lifetime.
    match in_job() {
        Ok(true) => {}
        Ok(false) => return refuse("The browser guard must run inside its App's Job Object."),
        Err(error) => return Err((2, format!("Job membership is unknown: {error}"))),
    }
    let Some(name) = std::env::var_os(STOP_EVENT_ENV) else {
        return refuse("The App's stop event is not named.");
    };
    let stop = Event::open(&name)
        .map_err(|error| (2, format!("The App's stop event cannot be opened: {error}")))?;
    let job = Job::kill_on_close()
        .map_err(|error| (2, format!("A Job Object cannot be created: {error}")))?;
    // Reserve the receipt before any browser can exist. CREATE_NEW refuses
    // a path created meanwhile, and a link there is opened as itself, never
    // followed. It inherits the runtime directory's access control.
    let receipt = OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(receipt)
        .map_err(|error| (2, format!("The receipt cannot be created: {error}")))?;
    Ok(Prepared { receipt, stop, job })
}

struct Run {
    job: Job,
    child: Option<Child>,
    root_exit: Option<i32>,
    forced: u32,
    cleanup_errors: BTreeSet<String>,
    non_fatal: BTreeSet<String>,
}

impl Run {
    /// Record the browser's exit code once it has exited.
    fn reap(&mut self) {
        if self.root_exit.is_some() {
            return;
        }
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                // Windows always has an exit code; NTSTATUS values wrap negative.
                Ok(Some(status)) => self.root_exit = status.code(),
                Ok(None) => {}
                Err(error) => {
                    self.cleanup_errors.insert(fault(&error));
                }
            }
        }
    }

    fn active(&mut self) -> Option<u32> {
        match self.job.processes() {
            Ok((_, active)) => Some(active),
            Err(error) => {
                self.cleanup_errors.insert(fault(&error));
                None
            }
        }
    }

    /// Whether the browser has exited and the job is empty.
    fn settled(&mut self) -> bool {
        self.reap();
        let browser_done = self.child.is_none() || self.root_exit.is_some();
        browser_done && self.active() == Some(0)
    }

    /// Wait for the browser to exit or for a stop request, without a
    /// lifetime cap. A failed wait is retried; one that keeps failing makes
    /// the guard drain.
    fn running(&mut self, stop: &Event) -> Result<(), String> {
        let Some(child) = &self.child else {
            return Ok(());
        };
        let handles = [child.as_raw_handle(), stop.as_raw_handle()];
        let mut faults = 0;
        loop {
            match wait_any(&handles, None) {
                // The browser exited, or the App asked the guard to stop.
                Ok(Some(_)) => break,
                Ok(None) => {}
                Err(error) => {
                    let error = fault(&error);
                    self.non_fatal.insert(error.clone());
                    faults += 1;
                    if faults >= MAX_RUNNING_FAULTS {
                        return Err(error);
                    }
                    std::thread::sleep(FAULT_PAUSE);
                }
            }
        }
        self.reap();
        Ok(())
    }

    /// Wait up to `budget` for the browser to exit and the job to empty.
    fn settle_within(&mut self, budget: Duration) -> bool {
        self.settle_until(Instant::now() + budget)
    }

    /// Wait until `deadline` for the browser to exit and the job to empty.
    fn settle_until(&mut self, deadline: Instant) -> bool {
        loop {
            if self.settled() {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let pause = DRAIN_TICK.min(deadline - now);
            // The browser's exit wakes this early.
            let woke = match &self.child {
                Some(child) if self.root_exit.is_none() => {
                    wait_any(&[child.as_raw_handle()], Some(pause)).map(|_| ())
                }
                _ => {
                    std::thread::sleep(pause);
                    Ok(())
                }
            };
            if woke.is_err() {
                std::thread::sleep(pause);
            }
        }
    }

    /// The bounded drain every run takes, however it ended. Returns whether
    /// the job was proven empty after the browser exited.
    fn drain(&mut self) -> bool {
        if self.natural() {
            return true;
        }
        // A job that emptied just as the natural budget ran out has nothing
        // left to end: no termination, and no forced cleanup is recorded.
        let before = self.job.processes();
        if matches!(before, Ok((_, 0))) {
            return self.settled() || self.settle_within(FORCED);
        }
        // What is in the job now, held open (so no id is reused meanwhile).
        // The total is read first: a process that enters between the two
        // reads is counted twice rather than missed.
        let listed = match self.job.process_ids() {
            Ok(listed) => listed,
            Err(error) => {
                self.cleanup_errors.insert(fault(&error));
                Vec::new()
            }
        };
        let held: Vec<(u32, io::Result<OwnedHandle>)> = listed
            .into_iter()
            .map(|pid| (pid, open_process(pid)))
            .collect();
        // Diagnostics only: who each one is, read while it still runs. This
        // lengthens the natural window by its time (`identifyMs`).
        let identifying = Instant::now();
        let identities: Vec<identity::Identity> = held
            .iter()
            .map(|(_, handle)| identity::identify(handle))
            .collect();
        let identify_ms = identifying.elapsed().as_millis();
        if injected("TERMINATE_LATE") {
            self.wait_empty();
        }
        if let Err(error) = self.job.terminate(FORCED_EXIT) {
            self.cleanup_errors.insert(fault(&error));
        }
        let forced_end = Instant::now() + FORCED;
        let empty = self.settle_until(forced_end);
        self.count_forced(
            held,
            identities,
            before,
            empty.then_some(forced_end),
            identify_ms,
        );
        empty
    }

    /// Forced cleanup, counted exactly: the listed processes `classify` finds
    /// forced (ended by the termination, or still running after it), and
    /// every process that entered the job after the total was read. A listed
    /// process with any other exit code, or that had already gone before it
    /// could be opened, ended by itself. A failed read leaves the count
    /// unknown: it is recorded in `cleanupErrors`, which makes the receipt
    /// abnormal, as a failed signal does on Linux. `empty_until` is the end
    /// of the drain's forced budget when its settle found the job empty:
    /// only then may a `STILL_ACTIVE`, unsignaled process get `SIGNAL_GRACE`,
    /// and never past that end, so the drain's outer bound is unchanged (with
    /// a job not found empty, such a process counts as forced, which can only
    /// over-count, on the safe side). The result is written on the guard's
    /// standard error (`identity::report`, diagnostics only).
    fn count_forced(
        &mut self,
        held: Vec<(u32, io::Result<OwnedHandle>)>,
        identities: Vec<identity::Identity>,
        before: io::Result<(u32, u32)>,
        empty_until: Option<Instant>,
        identify_ms: u128,
    ) {
        let mut named = Vec::new();
        let mut by_themselves = 0;
        for ((pid, handle), who) in held.into_iter().zip(&identities) {
            let grace = empty_until.map_or(Duration::ZERO, |end| {
                SIGNAL_GRACE.min(end.saturating_duration_since(Instant::now()))
            });
            let read = handle.and_then(|handle| exit_read(&handle, grace));
            let (code, signaled) = match &read {
                Ok((code, signaled)) => (Some(*code), *signaled),
                Err(error) if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER) => continue,
                Err(_) => (None, false),
            };
            match classify(code, signaled) {
                Ended::Forced => {
                    self.forced += 1;
                    let exit = code.filter(|code| *code != STILL_ACTIVE);
                    named.push(identity::entry(pid, who, exit));
                }
                Ended::ByItself => by_themselves += 1,
                Ended::Unknown => {
                    if let Err(error) = read {
                        self.cleanup_errors
                            .insert(format!("ForcedCleanupUnknown: {}", fault(&error)));
                    }
                }
            }
        }
        let mut entered = 0;
        match (before, self.job.processes()) {
            (Ok((before, _)), Ok((after, _))) => {
                entered = after.saturating_sub(before);
                self.forced += entered;
            }
            (Err(error), _) | (_, Err(error)) => {
                self.cleanup_errors
                    .insert(format!("ForcedCleanupUnknown: {}", fault(&error)));
            }
        }
        identity::report(self.forced, named, entered, by_themselves, identify_ms);
    }

    /// Self-test only: wait (bounded) until the job is empty.
    fn wait_empty(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.active() != Some(0) && Instant::now() < deadline {
            std::thread::sleep(DRAIN_TICK);
        }
    }

    /// The natural part of the drain. The self-test build can instead let
    /// the budget run out exactly as the job empties (`LATE_EMPTY`), which
    /// timing alone cannot produce on demand.
    fn natural(&mut self) -> bool {
        if injected("LATE_EMPTY") {
            self.wait_empty();
            return false;
        }
        self.settle_within(NATURAL)
    }
}

fn supervise(prepared: Prepared, program: &OsString, args: &[OsString]) -> ExitCode {
    let Prepared {
        mut receipt,
        stop,
        job,
    } = prepared;
    let mut run = Run {
        job,
        child: None,
        root_exit: None,
        forced: 0,
        cleanup_errors: BTreeSet::new(),
        non_fatal: BTreeSet::new(),
    };
    let mut failure = None;
    // A stop requested before the browser starts starts nothing.
    match stop.is_set() {
        Err(error) => failure = Some(fault(&error)),
        Ok(true) => failure = Some("StopRequestedBeforeStart".to_string()),
        Ok(false) => {
            let mut command = Command::new(program);
            command.args(args).env_remove(STOP_EVENT_ENV);
            match spawn_in(&run.job, &mut command, CREATE_NO_WINDOW) {
                Ok(child) => {
                    run.child = Some(child);
                    if let Err(error) = run.running(&stop) {
                        failure = Some(error);
                    }
                }
                Err(error) => failure = Some(fault(&error)),
            }
        }
    }
    let empty = run.drain();
    let complete = run.child.is_some() && run.root_exit.is_some() && empty;
    let normal =
        complete && run.root_exit == Some(0) && failure.is_none() && run.cleanup_errors.is_empty();
    let remaining = run.job.process_ids().unwrap_or_else(|error| {
        run.cleanup_errors.insert(fault(&error));
        Vec::new()
    });
    let observed = match run.job.processes() {
        Ok((total, _)) => total,
        Err(error) => {
            run.cleanup_errors.insert(fault(&error));
            0
        }
    };
    let normal = normal && run.cleanup_errors.is_empty();
    let proof = serde_json::json!({
        "rootExit": run.root_exit,
        "complete": complete,
        "remaining": remaining,
        "kernelNoChildren": empty,
        "observed": observed,
        "forcedCleanup": run.forced,
        "error": failure,
        "cleanupErrors": run.cleanup_errors,
        "nonFatalErrors": run.non_fatal,
    });
    // One write of the whole document, so a reader never sees a partial one
    // from a successful write.
    let written = serde_json::to_vec(&proof)
        .map_err(io::Error::from)
        .and_then(|bytes| receipt.write_all(&bytes))
        .and_then(|()| receipt.flush());
    if let Err(error) = written {
        eprintln!("takosumi-browser-guard: the receipt could not be written: {error}");
        return ExitCode::from(1);
    }
    if normal {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

#[cfg(test)]
#[path = "guard_windows_tests.rs"]
mod tests;
