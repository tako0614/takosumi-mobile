//! Product supervisor for one per-profile CEF browser process (macOS, M2).
//!
//! `takosumi-browser-guard <absolute new receipt> <program> [args...]`
//!
//! The same command line, receipt and exit codes as the Linux guard
//! (`guard_main.rs`) and the Windows guard (`guard_windows.rs`). macOS has
//! neither Linux's child subreaper and pidfds nor Windows' Job Objects, and
//! kqueue's `NOTE_TRACK` (following forks) is no longer supported, so the
//! guard owns the browser's processes by these means:
//!
//! - **Process group.** The App starts the guard as the leader of its own
//!   process group (it refuses otherwise); the browser and every process it
//!   starts inherit that group unless they leave it. Every member of the
//!   group other than the guard is the browser's (`proc_listpgrppids`).
//! - **Descendants, recorded while they run.** A descendant that leaves the
//!   group (`setsid`, `setpgid`) is still found as a child of a process the
//!   guard already tracks (`proc_listchildpids`). The browser's orphans are
//!   reparented to launchd, so the guard records every descendant
//!   continuously while the browser runs (each kqueue event, and at least
//!   every 250 ms) and keeps tracking it afterwards by pid and start time.
//! - **PID reuse.** Each tracked process is kept with its start time
//!   (`proc_pidinfo`, `PROC_PIDTBSDINFO`); one whose start time differs, or
//!   that started before the parent it was found under, is not the browser's
//!   and is never signalled. A zombie counts as ended.
//!
//! Not covered: a process that leaves the group and is orphaned between two
//! observations (under 250 ms, or before the next event) is not seen; a
//! process the browser has started through a system service (launchd, XPC)
//! is not its descendant, as on Linux one started by a session manager is
//! not. CEF's helpers stay in the group (the M1 smoke's group check).
//!
//! The guard waits on one kqueue, without a lifetime cap: the browser's exit
//! (`EVFILT_PROC`, `NOTE_EXIT`), a stop request (`SIGTERM` or `SIGINT`,
//! `EVFILT_SIGNAL`, as the App sends `SIGTERM` on Linux), and the App's death
//! (`NOTE_EXIT` on the guard's parent, the App: macOS has no parent-death
//! signal; a parent already gone at start is a stop request before the
//! browser starts). Both signals are ignored in the guard, which kqueue
//! still records, and restored to their defaults in the browser.
//!
//! The drain then always runs, as on Windows: up to 0.5 s for the browser
//! and every tracked process to end by themselves, then `SIGKILL` to each
//! tracked process still running (its start time checked right before) and
//! to every other member of the group, then up to 2 s. Forced cleanup counts
//! the processes the kill actually ended: the browser when `waitpid` reports
//! it ended by `SIGKILL`; any other process when it was running (not a
//! zombie) right before the kill, the kill succeeded and it was gone
//! afterwards. Such a process that ended by itself between that check and
//! the signal is counted forced: macOS gives the guard no exit status for a
//! process that is not its child, so the count can only err on the safe
//! side. The completion proof (`kernelNoChildren`) is that, after the
//! browser was reaped, the kernel lists no process in the guard's group but
//! the guard and no tracked process is alive.
//!
//! Receipt, written once after the drain, with the other guards' fields:
//! `{rootExit, complete, remaining, kernelNoChildren, observed, forcedCleanup,
//! error, cleanupErrors, nonFatalErrors}`. `remaining` lists the process ids
//! still in the group or tracked; `observed` counts every process seen (the
//! browser included, by pid and start time). `rootExit` is the browser's exit
//! code, or minus the signal that ended it. The guard exits 0 only when the
//! drain proved completion, the browser exited 0 and nothing failed; a
//! forced cleanup alone does not change the exit status, and the App
//! separately requires `forcedCleanup == 0`. A refusal before the receipt
//! exists exits 2 and starts nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

/// While the browser runs, observe its processes on every event and at
/// least this often.
const RUNNING_TICK: Duration = Duration::from_millis(250);
/// While draining, re-observe at least this often.
const DRAIN_TICK: Duration = Duration::from_millis(20);
/// Natural end, then SIGKILL and this long for everything to be gone.
const NATURAL: Duration = Duration::from_millis(500);
const FORCED: Duration = Duration::from_secs(2);
/// Consecutive failed waits before the guard gives up waiting and drains.
const MAX_RUNNING_FAULTS: u32 = 8;
/// The most processes one listing call returns; a listing that fills it is
/// read again with more room.
const LISTING: usize = 4096;

/// Called by `guard_entry.rs` on macOS.
pub(super) fn main() -> ExitCode {
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

/// A short, stable error name for the receipt; never a path or argv.
fn fault(error: &io::Error) -> String {
    match error.raw_os_error() {
        Some(code) => format!("{:?} (errno {code})", error.kind()),
        None => format!("{:?}", error.kind()),
    }
}

/// What the kernel says about one process: its parent, group, start time
/// (microseconds since the epoch) and whether it is a zombie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Facts {
    ppid: i32,
    pgid: i32,
    start: u64,
    zombie: bool,
}

/// `proc_pidinfo(PROC_PIDTBSDINFO)`; `None` when no such process exists.
fn facts(pid: i32) -> io::Result<Option<Facts>> {
    // SAFETY: proc_bsdinfo is plain data; the call writes at most its size.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    if read == size {
        return Ok(Some(Facts {
            ppid: info.pbi_ppid as i32,
            pgid: info.pbi_pgid as i32,
            start: info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
            zombie: info.pbi_status == libc::SZOMB,
        }));
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH) | None | Some(0) => Ok(None),
        _ => Err(error),
    }
}

/// A process list call: `proc_listpgrppids` or `proc_listchildpids`.
type Listing = unsafe extern "C" fn(libc::pid_t, *mut libc::c_void, libc::c_int) -> libc::c_int;

fn list(call: Listing, id: i32) -> io::Result<Vec<i32>> {
    let mut room = LISTING;
    loop {
        let mut pids = vec![0 as libc::pid_t; room];
        let bytes = (room * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: the buffer holds `room` pids and its size is passed.
        let count = unsafe { call(id, pids.as_mut_ptr().cast(), bytes) };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        let count = count as usize;
        if count >= room {
            room *= 2;
            continue;
        }
        pids.truncate(count);
        pids.retain(|pid| *pid > 0);
        return Ok(pids);
    }
}

/// The members of a process group.
fn group(pgid: i32) -> io::Result<Vec<i32>> {
    list(libc::proc_listpgrppids, pgid)
}

/// The children of a process.
fn children(pid: i32) -> io::Result<Vec<i32>> {
    list(libc::proc_listchildpids, pid)
}

/// Whether a process found under `parent_start` (its parent's start time)
/// can be that parent's: it must have started no earlier. A reused pid
/// whose old owner was the parent's child is newer, so this is necessary,
/// not sufficient; the group and the tracked start time do the rest.
fn plausible_child(child_start: u64, parent_start: u64) -> bool {
    child_start >= parent_start
}

/// Whether a tracked process (`recorded` start time) is still the same
/// running process: same start time and not a zombie.
fn still_running(recorded: u64, now: Option<Facts>) -> bool {
    matches!(now, Some(facts) if facts.start == recorded && !facts.zombie)
}

/// A wait status as the receipt's `rootExit`: the exit code, or minus the
/// signal that ended the process.
fn exit_code(status: libc::c_int) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        -libc::WTERMSIG(status)
    } else {
        status
    }
}

/// One kqueue: the browser's exit, the App's exit and stop signals.
struct Queue(OwnedFd);

/// What woke the guard.
#[derive(Default)]
struct Woke {
    browser_exited: bool,
    stop: bool,
}

impl Queue {
    fn new() -> io::Result<Self> {
        // SAFETY: no arguments; a kqueue is not inherited across fork.
        let fd = unsafe { libc::kqueue() };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a fresh descriptor this process owns.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    fn change(&self, ident: usize, filter: i16, fflags: u32) -> io::Result<()> {
        let change = libc::kevent {
            ident,
            filter,
            flags: libc::EV_ADD | libc::EV_ENABLE | libc::EV_CLEAR,
            fflags,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        // SAFETY: one valid change, no event buffer.
        let added = unsafe {
            libc::kevent(
                self.0.as_raw_fd(),
                &change,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        if added < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn watch_exit(&self, pid: i32) -> io::Result<()> {
        self.change(pid as usize, libc::EVFILT_PROC, libc::NOTE_EXIT)
    }

    fn watch_signal(&self, signal: libc::c_int) -> io::Result<()> {
        self.change(signal as usize, libc::EVFILT_SIGNAL, 0)
    }

    /// Wait up to `limit` for events; `browser` is the browser's pid and
    /// `parent` the App's.
    fn wait(&self, limit: Duration, browser: Option<i32>, parent: i32) -> io::Result<Woke> {
        let timeout = libc::timespec {
            tv_sec: limit.as_secs() as libc::time_t,
            tv_nsec: limit.subsec_nanos() as libc::c_long,
        };
        // SAFETY: kevent is plain data; the buffer's length is passed.
        let mut events: [libc::kevent; 8] = unsafe { std::mem::zeroed() };
        let count = unsafe {
            libc::kevent(
                self.0.as_raw_fd(),
                std::ptr::null(),
                0,
                events.as_mut_ptr(),
                events.len() as libc::c_int,
                &timeout,
            )
        };
        if count < 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::EINTR) {
                Ok(Woke::default())
            } else {
                Err(error)
            };
        }
        let mut woke = Woke::default();
        for event in &events[..count as usize] {
            match event.filter {
                libc::EVFILT_SIGNAL => woke.stop = true,
                libc::EVFILT_PROC if Some(event.ident as i32) == browser => {
                    woke.browser_exited = true
                }
                libc::EVFILT_PROC if event.ident as i32 == parent => woke.stop = true,
                _ => {}
            }
        }
        Ok(woke)
    }
}

/// Mark every inherited descriptor but standard input, output and error
/// close-on-exec, so the browser inherits only the pipes and the log the App
/// passed. macOS lists a process's open descriptors in `/dev/fd`.
fn close_inherited_on_exec() -> io::Result<()> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/dev/fd")? {
        if let Some(fd) = entry?
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        {
            found.push(fd);
        }
    }
    for fd in found.into_iter().filter(|fd| *fd > 2) {
        // SAFETY: fcntl on a descriptor number; a closed one fails with
        // EBADF (the listing's own descriptor, already closed).
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags >= 0 && unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

struct Prepared {
    receipt: File,
    queue: Queue,
    parent: i32,
    stopping: bool,
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
    // SAFETY: getpgrp and getpid have no preconditions.
    if unsafe { libc::getpgrp() != libc::getpid() } {
        return refuse("The browser guard must lead its own process group.");
    }
    if let Err(error) = close_inherited_on_exec() {
        return Err((
            2,
            format!("Inherited descriptors cannot be withheld: {error}"),
        ));
    }
    let queue =
        Queue::new().map_err(|error| (2, format!("A kqueue cannot be created: {error}")))?;
    // Reserve the receipt before any browser can exist. O_EXCL refuses a path
    // or link created meanwhile; the explicit mode survives any umask.
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(receipt)
        .map_err(|error| (2, format!("The receipt cannot be created: {error}")))?;
    let fail = |error: io::Error| (1, format!("Cannot guard the browser lifecycle: {error}"));
    // Stop requests: ignored here (kqueue records them anyway), default in
    // the browser. SIGCHLD keeps its default, so the browser's exit status
    // is not discarded.
    // SAFETY: plain signal dispositions in a single-threaded process.
    unsafe {
        for (signal, action) in [
            (libc::SIGTERM, libc::SIG_IGN),
            (libc::SIGINT, libc::SIG_IGN),
            (libc::SIGCHLD, libc::SIG_DFL),
        ] {
            if libc::signal(signal, action) == libc::SIG_ERR {
                return Err(fail(io::Error::last_os_error()));
            }
        }
    }
    for signal in [libc::SIGTERM, libc::SIGINT] {
        queue.watch_signal(signal).map_err(fail)?;
    }
    // The App's death: its exit, watched from now on. A parent already gone
    // (this guard was reparented to launchd) is a stop before the start.
    // SAFETY: getppid has no preconditions.
    let parent = unsafe { libc::getppid() };
    let stopping = parent == 1
        || match queue.watch_exit(parent) {
            Ok(()) => false,
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => true,
            Err(error) => return Err(fail(error)),
        };
    Ok(Prepared {
        receipt: file,
        queue,
        parent,
        stopping,
    })
}

/// The browser's command: its default stop signals and an empty signal mask,
/// whatever the guard set.
fn browser_command(program: &OsString, args: &[OsString]) -> Command {
    let mut command = Command::new(program);
    command.args(args);
    // SAFETY: runs in the forked child before exec and makes only
    // async-signal-safe calls (signal, sigprocmask).
    unsafe {
        command.pre_exec(|| {
            for signal in [libc::SIGTERM, libc::SIGINT] {
                if libc::signal(signal, libc::SIG_DFL) == libc::SIG_ERR {
                    return Err(io::Error::last_os_error());
                }
            }
            let mut none: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut none);
            if libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
}

struct Run {
    me: i32,
    queue: Queue,
    parent: i32,
    child: Option<i32>,
    root_exit: Option<i32>,
    /// Processes of the browser's still running, with their start times.
    tracked: BTreeMap<i32, u64>,
    /// Every process seen, by pid and start time (the browser included).
    observed: BTreeSet<(i32, u64)>,
    forced: u32,
    cleanup_errors: BTreeSet<String>,
    non_fatal: BTreeSet<String>,
}

impl Run {
    /// Record the browser's exit status once it has exited.
    fn reap(&mut self) {
        let Some(child) = self.child else {
            return;
        };
        if self.root_exit.is_some() {
            return;
        }
        let mut status = 0;
        // SAFETY: waitpid on this guard's own child.
        match unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) } {
            0 => {}
            pid if pid == child => {
                self.root_exit = Some(exit_code(status));
                self.tracked.remove(&child);
            }
            _ => {
                let error = io::Error::last_os_error();
                self.cleanup_errors.insert(fault(&error));
            }
        }
    }

    fn track(&mut self, pid: i32, facts: Facts) {
        if pid != self.me && !facts.zombie {
            self.tracked.entry(pid).or_insert(facts.start);
            self.observed.insert((pid, facts.start));
        }
    }

    /// Record the browser's processes now: every member of the group, and
    /// the children of every tracked process; drop those that ended or
    /// whose pid now names another process. Returns the first failure.
    fn observe(&mut self) -> Result<(), String> {
        let mut problem = None;
        match group(self.me) {
            Ok(members) => {
                for pid in members {
                    match facts(pid) {
                        Ok(Some(facts)) if facts.pgid == self.me => self.track(pid, facts),
                        Ok(_) => {}
                        Err(error) => problem = Some(fault(&error)),
                    }
                }
            }
            Err(error) => problem = Some(fault(&error)),
        }
        // Children of tracked processes, to any depth: one that left the
        // group is still the browser's while its parent is.
        let mut pending: Vec<(i32, u64)> = self
            .tracked
            .iter()
            .map(|(pid, start)| (*pid, *start))
            .collect();
        let mut visited = BTreeSet::new();
        while let Some((parent, parent_start)) = pending.pop() {
            if !visited.insert(parent) || visited.len() > LISTING {
                continue;
            }
            match children(parent) {
                Ok(found) => {
                    for pid in found {
                        match facts(pid) {
                            Ok(Some(facts))
                                if facts.ppid == parent
                                    && plausible_child(facts.start, parent_start) =>
                            {
                                let new = !self.tracked.contains_key(&pid);
                                self.track(pid, facts);
                                if new && !facts.zombie {
                                    pending.push((pid, facts.start));
                                }
                            }
                            Ok(_) => {}
                            Err(error) => problem = Some(fault(&error)),
                        }
                    }
                }
                Err(error) => problem = Some(fault(&error)),
            }
        }
        let tracked: Vec<(i32, u64)> = self
            .tracked
            .iter()
            .map(|(pid, start)| (*pid, *start))
            .collect();
        for (pid, start) in tracked {
            // The unreaped browser keeps its pid until it is reaped.
            if Some(pid) == self.child && self.root_exit.is_none() {
                continue;
            }
            match facts(pid) {
                Ok(now) if still_running(start, now) => {}
                Ok(_) => {
                    self.tracked.remove(&pid);
                }
                Err(error) => problem = Some(fault(&error)),
            }
        }
        problem.map_or(Ok(()), Err)
    }

    /// The group's members other than the guard.
    fn group_left(&mut self) -> Option<Vec<i32>> {
        match group(self.me) {
            Ok(mut members) => {
                members.retain(|pid| *pid != self.me);
                // A zombie whose parent is gone is reaped by launchd; it no
                // longer runs.
                members.retain(|pid| matches!(facts(*pid), Ok(Some(facts)) if !facts.zombie));
                Some(members)
            }
            Err(error) => {
                self.cleanup_errors.insert(fault(&error));
                None
            }
        }
    }

    /// Whether the browser was reaped, no tracked process runs and the group
    /// holds only the guard.
    fn settled(&mut self) -> bool {
        self.reap();
        if let Err(error) = self.observe() {
            self.cleanup_errors.insert(error);
        }
        let browser_done = self.child.is_none() || self.root_exit.is_some();
        browser_done
            && self.tracked.is_empty()
            && self.group_left().is_some_and(|left| left.is_empty())
    }

    /// Wait for the browser to exit or a stop request (also the App's
    /// death), observing on every event and every tick.
    fn running(&mut self) -> Result<(), String> {
        let Some(child) = self.child else {
            return Ok(());
        };
        let mut faults = 0;
        loop {
            match self.queue.wait(RUNNING_TICK, Some(child), self.parent) {
                Ok(woke) => {
                    faults = 0;
                    if let Err(error) = self.observe() {
                        self.non_fatal.insert(error);
                    }
                    self.reap();
                    if woke.stop || woke.browser_exited || self.root_exit.is_some() {
                        return Ok(());
                    }
                }
                Err(error) => {
                    let error = fault(&error);
                    self.non_fatal.insert(error.clone());
                    faults += 1;
                    if faults >= MAX_RUNNING_FAULTS {
                        return Err(error);
                    }
                    std::thread::sleep(RUNNING_TICK);
                }
            }
        }
    }

    /// Wait until `deadline` for `settled`.
    fn settle_until(&mut self, deadline: Instant) -> bool {
        loop {
            if self.settled() {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            std::thread::sleep(DRAIN_TICK.min(deadline - now));
        }
    }

    /// The bounded drain every run takes. Returns whether completion was
    /// proven.
    fn drain(&mut self) -> bool {
        if self.settle_until(Instant::now() + NATURAL) {
            return true;
        }
        // Who runs now: each tracked process (its start time checked right
        // before its signal) and every other member of the group.
        let mut killed: Vec<(i32, u64)> = Vec::new();
        let mut targets: BTreeMap<i32, Option<u64>> = self
            .tracked
            .iter()
            .map(|(pid, start)| (*pid, Some(*start)))
            .collect();
        for pid in self.group_left().unwrap_or_default() {
            targets.entry(pid).or_insert(None);
        }
        let mut child_killed = false;
        for (pid, start) in targets {
            if Some(pid) == self.child {
                if self.root_exit.is_none() {
                    // An unreaped child cannot have its pid reused; its own
                    // wait status says whether the kill ended it.
                    // SAFETY: kill with a plain pid and signal.
                    if unsafe { libc::kill(pid, libc::SIGKILL) } == 0 {
                        child_killed = true;
                    }
                }
                continue;
            }
            let now = match facts(pid) {
                Ok(now) => now,
                Err(error) => {
                    self.cleanup_errors.insert(fault(&error));
                    continue;
                }
            };
            let current = match (start, now) {
                (Some(recorded), now) if still_running(recorded, now) => recorded,
                // A group member seen only now (not tracked yet).
                (None, Some(facts)) if !facts.zombie && facts.pgid == self.me => facts.start,
                _ => continue,
            };
            // SAFETY: kill with a plain pid and signal; checked just above.
            if unsafe { libc::kill(pid, libc::SIGKILL) } == 0 {
                killed.push((pid, current));
                self.observed.insert((pid, current));
            } else {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    self.cleanup_errors.insert(fault(&error));
                }
            }
        }
        let empty = self.settle_until(Instant::now() + FORCED);
        // The browser: its wait status says whether the kill ended it.
        if child_killed && self.root_exit == Some(-libc::SIGKILL) {
            self.forced += 1;
        }
        // Any other process: it ran right before a kill that succeeded. One
        // still running after the forced budget counts too (the run is then
        // not complete).
        self.forced += killed.len() as u32;
        empty
    }
}

fn supervise(prepared: Prepared, program: &OsString, args: &[OsString]) -> ExitCode {
    let Prepared {
        mut receipt,
        queue,
        parent,
        stopping,
    } = prepared;
    // SAFETY: getpid has no preconditions.
    let me = unsafe { libc::getpid() };
    let mut run = Run {
        me,
        queue,
        parent,
        child: None,
        root_exit: None,
        tracked: BTreeMap::new(),
        observed: BTreeSet::new(),
        forced: 0,
        cleanup_errors: BTreeSet::new(),
        non_fatal: BTreeSet::new(),
    };
    let mut failure = None;
    if stopping {
        failure = Some("StopRequestedBeforeStart".to_string());
    } else {
        match browser_command(program, args).spawn() {
            Ok(child) => {
                let pid = child.id() as i32;
                // The guard reaps it itself (waitpid); std's handle is not
                // waited on.
                std::mem::forget(child);
                run.child = Some(pid);
                match facts(pid) {
                    Ok(Some(facts)) => {
                        run.tracked.insert(pid, facts.start);
                        run.observed.insert((pid, facts.start));
                    }
                    _ => {
                        run.observed.insert((pid, 0));
                    }
                }
                match run.queue.watch_exit(pid) {
                    Ok(()) => {}
                    // Already exited: the reap below records it.
                    Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {}
                    Err(error) => {
                        run.non_fatal.insert(fault(&error));
                    }
                }
                if let Err(error) = run.running() {
                    failure = Some(error);
                }
            }
            Err(error) => failure = Some(fault(&error)),
        }
    }
    let empty = run.drain();
    let complete = run.child.is_some() && run.root_exit.is_some() && empty;
    let mut remaining: BTreeSet<i32> = run.tracked.keys().copied().collect();
    remaining.extend(run.group_left().unwrap_or_default());
    let normal =
        complete && run.root_exit == Some(0) && failure.is_none() && run.cleanup_errors.is_empty();
    let proof = serde_json::json!({
        "rootExit": run.root_exit,
        "complete": complete,
        "remaining": remaining,
        "kernelNoChildren": empty,
        "observed": run.observed.len(),
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
#[path = "guard_macos_tests.rs"]
mod tests;
