//! Product supervisor for one per-profile CEF browser process (Linux only).
//!
//! `takosumi-browser-guard <absolute new receipt> <program> [args...]`
//!
//! The App starts this guard in its own process group from a dedicated
//! long-lived thread. The guard starts the browser `program`, owns every
//! process the browser leaves behind, and writes one JSON receipt when it
//! ends. It keeps the contract and receipt of the fixture guard
//! `native-tests/browser/guard.py`, with two intended differences: the
//! browser's lifetime is not capped, and while the browser runs the guard
//! blocks on a signalfd (stop requests and child exits) with a 250 ms
//! observation tick instead of polling every 20 ms. Because it may run for
//! days, it keeps descriptors only for processes that still exist, and an
//! observation failure never ends a running browser. Smaller hardening
//! details are noted where they occur.
//!
//! Receipt, written once after the drain:
//! `{rootExit, complete, remaining, kernelNoChildren, observed, forcedCleanup,
//! error, cleanupErrors, nonFatalErrors}`. The guard exits 0 only when the
//! drain ended with the kernel's proof of no children, the browser exited 0
//! and nothing failed. As in guard.py, a forced cleanup alone does not change
//! the exit status; the App separately requires `forcedCleanup == 0` for a
//! normal end. `nonFatalErrors` names failures while the browser ran
//! (observation, the browser's pidfd, a reap or wait that recovered); they did
//! not stop it, and the drain's own observation and ECHILD still decide
//! completeness. A refusal before the receipt exists exits 2 and starts
//! nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{File, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

/// While the browser runs, observe descendants on every signal and at least
/// this often. Ownership is proven by ECHILD at the end, not by this tick.
const RUNNING_TICK: Duration = Duration::from_millis(250);
/// While draining, re-observe and re-signal at least this often.
const DRAIN_TICK: Duration = Duration::from_millis(20);
/// Natural exit, then SIGTERM, then SIGKILL; ECHILD ends the drain early.
const DRAIN: [(Option<libc::c_int>, Duration); 3] = [
    (None, Duration::from_millis(500)),
    (Some(libc::SIGTERM), Duration::from_millis(500)),
    (Some(libc::SIGKILL), Duration::from_secs(2)),
];
const MAX_ANCESTRY: usize = 128;
/// Consecutive running ticks in which reaping or waiting may fail before the
/// guard gives up watching and drains (about 2 s). A shorter failure is
/// recorded in `nonFatalErrors` and the browser keeps running.
const MAX_RUNNING_FAULTS: u32 = 8;

/// The Android fixture entry can allow its SDK helper one clean-exit poll.
/// The product entry keeps the standard drain.
pub(super) fn main(clean_exit_grace: Option<Duration>) -> ExitCode {
    // The self-test build runs its tests unless it is asked to act as the
    // guard; the release binary has no such switch.
    #[cfg(test)]
    if std::env::var_os(tests::AS_GUARD).is_none() {
        return tests::run();
    }
    let args: Vec<OsString> = std::env::args_os().collect();
    match prepare(&args) {
        Ok(prepared) => supervise(prepared, &args[2], &args[3..], clean_exit_grace),
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

/// The process ended (or was reaped) while it was being read.
fn gone(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(libc::ESRCH)
}

/// (start time, parent pid) from `/proc/<pid>/stat`. The fields are split at
/// the *last* ") " because the process name itself may contain ") ". The
/// start time (clock ticks after boot) tells a process apart from a later one
/// that reuses its pid.
fn parse_stat(text: &str) -> Option<(u64, i32)> {
    let fields: Vec<&str> = text.rsplit_once(") ")?.1.split_whitespace().collect();
    Some((fields.get(19)?.parse().ok()?, fields.get(1)?.parse().ok()?))
}

// COMM_CLASS_OBSERVATION_BEGIN
/// A spoofable process-name class, never a role, ownership or signal policy.
/// No raw comm is retained. Linux truncates comm to 15 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommClass {
    Unknown = 0,
    Other = 1,
    Emulator = 2,
    Qemu = 3,
    Crashpad = 4,
    Adb = 5,
}

const COMM_CLASSES: [&str; 6] = [
    "unknown",
    "other",
    "emulator_name",
    "qemu_name",
    "crashpad_name",
    "adb_name",
];

fn comm_class(text: &str) -> CommClass {
    let Some((_, name)) = text.split_once(" (") else {
        return CommClass::Unknown;
    };
    let Some((name, _)) = name.rsplit_once(") ") else {
        return CommClass::Unknown;
    };
    match name {
        "" => CommClass::Unknown,
        "emulator" => CommClass::Emulator,
        "qemu-system-x86" | "qemu-system-aar" => CommClass::Qemu,
        "crashpad_handle" | "crashpad" => CommClass::Crashpad,
        "adb" => CommClass::Adb,
        _ => CommClass::Other,
    }
}

struct StatSnapshot {
    start: u64,
    parent: i32,
    comm: CommClass,
}
// COMM_CLASS_OBSERVATION_END

/// (start time, parent pid), or None once the process is gone.
fn stat_snapshot(pid: i32) -> Result<Option<StatSnapshot>, String> {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(text) => parse_stat(&text)
            .map(|(start, parent)| StatSnapshot {
                start,
                parent,
                comm: comm_class(&text),
            })
            .map(Some)
            .ok_or_else(|| "MalformedStat".to_string()),
        Err(error) if gone(&error) => Ok(None),
        Err(error) => Err(fault(&error)),
    }
}

fn ancestry(pid: i32) -> Result<Option<(u64, i32)>, String> {
    Ok(stat_snapshot(pid)?.map(|stat| (stat.start, stat.parent)))
}

fn start_time(pid: i32) -> Result<Option<u64>, String> {
    Ok(ancestry(pid)?.map(|(start, _)| start))
}

/// Every process below `root` in one /proc snapshot, read from each thread's
/// `children` list (a child belongs to the thread that forked it). The
/// snapshot may be stale by the time it is used; `Tracker::observe` re-checks
/// each pid before retaining it.
fn descendants(root: i32) -> Result<BTreeSet<i32>, String> {
    let mut found = BTreeSet::new();
    let mut pending = vec![root];
    while let Some(pid) = pending.pop() {
        let tasks = match std::fs::read_dir(format!("/proc/{pid}/task")) {
            Ok(tasks) => tasks,
            Err(error) if gone(&error) => continue,
            Err(error) => return Err(fault(&error)),
        };
        for task in tasks {
            let task = match task {
                Ok(task) => task,
                Err(error) if gone(&error) => continue,
                Err(error) => return Err(fault(&error)),
            };
            let children = match std::fs::read_to_string(task.path().join("children")) {
                Ok(children) => children,
                Err(error) if gone(&error) => continue,
                Err(error) => return Err(fault(&error)),
            };
            for word in children.split_whitespace() {
                let child = word
                    .parse::<i32>()
                    .map_err(|_| "MalformedChildren".to_string())?;
                // The visited set also ends a transient cycle from a reused pid.
                if child != root && found.insert(child) {
                    pending.push(child);
                }
            }
        }
    }
    Ok(found)
}

fn pidfd_open(pid: i32) -> io::Result<OwnedFd> {
    // SAFETY: integer-only syscall. On success the kernel returns a new
    // close-on-exec descriptor that nothing else owns.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd as RawFd) })
}

/// Self-test fault injection, compiled into the self-test binary only; in a
/// release build every call is a constant `false`.
#[cfg(test)]
fn injected(point: &str) -> bool {
    tests::inject(point)
}
#[cfg(not(test))]
#[inline(always)]
fn injected(_point: &str) -> bool {
    false
}

fn injected_error() -> io::Error {
    io::Error::from_raw_os_error(libc::ENOMEM)
}

/// The direct child's pidfd.
fn child_pidfd(pid: i32) -> io::Result<OwnedFd> {
    if injected("CHILD_PIDFD") {
        return Err(injected_error());
    }
    pidfd_open(pid)
}

// TERMINAL_SIGNAL_ELIGIBILITY_BEGIN
/// Only a known terminal notification permits skipping a nonzero signal.
/// These descriptors come from pidfd_open(flags=0), so readiness covers the
/// whole thread group. Errors, invalid fds and unknown events stay eligible.
fn pidfd_terminal_ready(polled: libc::c_int, events: libc::c_short) -> bool {
    polled == 1
        && events & libc::POLLIN != 0
        && events & !(libc::POLLIN | libc::POLLRDNORM | libc::POLLHUP) == 0
}

// READINESS_OBSERVATION_BEGIN
#[derive(Clone, Copy)]
struct PidfdReadiness {
    polled: libc::c_int,
    revents: libc::c_short,
    terminal: bool,
    poll_errno: Option<i32>,
}

fn pidfd_readiness_with(
    fd: RawFd,
    poll: impl FnOnce(&mut libc::pollfd) -> libc::c_int,
) -> PidfdReadiness {
    let mut entry = libc::pollfd {
        fd,
        events: libc::POLLIN | libc::POLLRDNORM,
        revents: 0,
    };
    let polled = poll(&mut entry);
    // Capture the failing poll's errno before classification or a later signal.
    let poll_errno = if polled < 0 {
        io::Error::last_os_error().raw_os_error()
    } else {
        None
    };
    PidfdReadiness {
        polled,
        revents: entry.revents,
        terminal: pidfd_terminal_ready(polled, entry.revents),
        poll_errno,
    }
}

fn pidfd_readiness(fd: &OwnedFd) -> PidfdReadiness {
    // SAFETY: one valid pollfd, the same nonblocking timeout, no ownership change.
    pidfd_readiness_with(fd.as_raw_fd(), |entry| unsafe { libc::poll(entry, 1, 0) })
}

fn signal_readiness<T>(
    signal: libc::c_int,
    fd: Option<&T>,
    poll: impl FnOnce(&T) -> PidfdReadiness,
) -> Option<PidfdReadiness> {
    if signal == 0 {
        None
    } else {
        fd.map(poll)
    }
}
// READINESS_OBSERVATION_END
// TERMINAL_SIGNAL_ELIGIBILITY_END

fn pidfd_signal(fd: &OwnedFd, signal: libc::c_int) -> io::Result<()> {
    // SAFETY: a live owned pidfd, a null siginfo and no flags.
    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            fd.as_raw_fd(),
            signal,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Whether the process behind a pidfd still exists, zombies included, without
/// opening anything (so it works at the descriptor limit): signal 0 checks
/// existence and permission only. EPERM means it exists under other
/// credentials; only ESRCH (reaped) means gone.
fn exists(fd: &OwnedFd) -> bool {
    match pidfd_signal(fd, 0) {
        Ok(()) => true,
        Err(error) => error.raw_os_error() != Some(libc::ESRCH),
    }
}

/// Whether `pid` descends from the live guard `owner` (start `owner_start`)
/// through a parent chain that reads the same twice.
///
/// A pid from the snapshot can be stale: its process may have exited and the
/// pid been reused by an unrelated process. Walk parent links up to the owner,
/// then re-read every link from the owner downward. Each link must show the
/// same start time and parent both times, so each process on the chain stayed
/// the same process with the same parent across the walk, and the chain ends
/// at this exact guard. Orphans are only ever reparented toward the guard (the
/// nearest subreaper), never out of its subtree, so a stable chain proves
/// ownership now. No argv, UID or name is consulted.
fn is_owned(pid: i32, owner: i32, owner_start: u64) -> Result<bool, String> {
    let mut chain: Vec<(i32, (u64, i32))> = Vec::new();
    let mut current = pid;
    for _ in 0..MAX_ANCESTRY {
        let Some(link) = ancestry(current)? else {
            return Ok(false);
        };
        chain.push((current, link));
        if current == owner {
            if link.0 != owner_start {
                return Ok(false);
            }
            for (item, expected) in chain.iter().rev() {
                if ancestry(*item)? != Some(*expected) {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        let parent = link.1;
        if parent <= 1 || chain.iter().any(|(item, _)| *item == parent) {
            return Ok(false);
        }
        current = parent;
    }
    Ok(false)
}

/// Descendants retained by pidfd, keyed by pid, with the start time verified
/// for that pidfd. Signals go through the pidfd, so a reused pid is never hit.
#[derive(Default)]
struct Tracker {
    // The class comes only from a matching-birth stat snapshot; it can become
    // stale or be spoofed. It follows the pidfd's lifetime, never a reaped PID.
    handles: BTreeMap<i32, (u64, OwnedFd, CommClass)>,
    /// Distinct owned processes retained so far; it only grows.
    observed: u64,
}

impl Tracker {
    fn observe(&mut self, owner: i32) -> Result<(), String> {
        // Release the handles of processes that have been reaped. Only
        // processes that still exist, zombies included, keep a pidfd, so the
        // guard's descriptors follow the live process tree however many
        // renderers come and go. A gone process never reappears in a
        // snapshot, so it is never counted twice. The check uses the pidfd
        // itself, so pruning still works when no descriptor is free.
        self.handles.retain(|_, (_, fd, _)| exists(fd));
        if injected("OBSERVE") {
            return Err(fault(&injected_error()));
        }
        let owner_start = start_time(owner)?.ok_or("IdentityUnavailable")?;
        for pid in descendants(owner)? {
            let Some(snapshot) = stat_snapshot(pid)? else {
                continue;
            };
            let start = snapshot.start;
            match self.handles.get_mut(&pid) {
                Some((known, _, class)) if *known == start => {
                    *class = snapshot.comm;
                    continue;
                }
                // A kept handle's process still exists and so still holds
                // its pid; a different start time here can only mean a
                // stale read. Replace the handle rather than trust either.
                Some(_) => {
                    self.handles.remove(&pid);
                }
                None => {}
            }
            let fd = match pidfd_open(pid) {
                Ok(fd) => fd,
                Err(error) if error.raw_os_error() == Some(libc::ESRCH) => continue,
                Err(error) => return Err(fault(&error)),
            };
            // The pidfd was opened after the snapshot, so it might name a
            // later process that reused the pid. Keep it only if the pid still
            // shows the start time read before opening (so the pidfd names
            // that same process) and that process still descends from this
            // exact live guard. Otherwise, or on a read error, `fd` closes.
            if let Some(verified) = stat_snapshot(pid)? {
                if verified.start == start && is_owned(pid, owner, owner_start)? {
                    self.handles.insert(pid, (start, fd, verified.comm));
                    self.observed += 1;
                }
            }
        }
        Ok(())
    }

    /// Retained processes that still exist (zombies included).
    fn alive(&self) -> Vec<i32> {
        self.handles
            .iter()
            .filter(|(_, (_, fd, _))| exists(fd))
            .map(|(pid, _)| *pid)
            .collect()
    }
}

/// SIGTERM, SIGINT and SIGCHLD are blocked and read from a signalfd, so a stop
/// request or a child exit wakes a blocking poll.
struct Signals(OwnedFd);

impl Signals {
    fn block() -> io::Result<Self> {
        // SAFETY: sigset_t is plain data initialised by sigemptyset; the guard
        // is single-threaded, so the process mask is this thread's mask.
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGCHLD] {
                libc::sigaddset(&mut set, signal);
            }
            if libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
            // Ignored dispositions survive exec. An inherited SIG_IGN for
            // SIGCHLD would make the kernel reap children itself, losing the
            // browser's exit status and freeing its pid; guard.py's handlers
            // also give the browser default SIGTERM and SIGINT. Blocked
            // signals still reach the signalfd whatever the disposition.
            for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGCHLD] {
                if libc::signal(signal, libc::SIG_DFL) == libc::SIG_ERR {
                    return Err(io::Error::last_os_error());
                }
            }
            let fd = libc::signalfd(-1, &set, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK);
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(OwnedFd::from_raw_fd(fd)))
        }
    }

    /// Wait up to `limit` for a signal, then consume every pending one.
    /// Returns whether a stop (SIGTERM or SIGINT) was among them.
    fn wait(&self, limit: Duration) -> io::Result<bool> {
        let mut ready = libc::pollfd {
            fd: self.0.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // Round up so a short remainder still sleeps instead of spinning.
        let millis = limit.as_micros().div_ceil(1000).min(i32::MAX as u128) as libc::c_int;
        // SAFETY: one valid pollfd.
        if unsafe { libc::poll(&mut ready, 1, millis) } < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINTR) {
                return Err(error);
            }
        }
        let mut stop = false;
        loop {
            // SAFETY: signalfd_siginfo is plain data and the read is bounded
            // by its size; the descriptor is non-blocking.
            let mut info: libc::signalfd_siginfo = unsafe { std::mem::zeroed() };
            let size = std::mem::size_of::<libc::signalfd_siginfo>();
            let read = unsafe {
                libc::read(
                    self.0.as_raw_fd(),
                    (&mut info as *mut libc::signalfd_siginfo).cast(),
                    size,
                )
            };
            if read < 0 {
                let error = io::Error::last_os_error();
                match error.raw_os_error() {
                    Some(libc::EAGAIN) => return Ok(stop),
                    Some(libc::EINTR) => continue,
                    _ => return Err(error),
                }
            }
            if read as usize != size {
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
            }
            let signal = info.ssi_signo as libc::c_int;
            stop |= signal == libc::SIGTERM || signal == libc::SIGINT;
        }
    }
}

/// Refuse before any receipt or browser exists if this kernel lacks what the
/// guard relies on: pidfds that can signal, and per-thread /proc `children`
/// lists (CONFIG_PROC_CHILDREN), checked on the guard itself. Without them the
/// observation below would silently see no descendants.
fn probe() -> io::Result<()> {
    // SAFETY: getpid has no preconditions.
    let me = unsafe { libc::getpid() };
    pidfd_signal(&pidfd_open(me)?, 0)?;
    std::fs::read_to_string(format!("/proc/{me}/task/{me}/children"))?;
    Ok(())
}

/// The App may leak inheritable descriptors into the guard; the browser must
/// not receive them (guard.py's Popen uses close_fds). Mark every descriptor
/// above stderr close-on-exec in the guard itself before anything starts:
/// the guard never uses inherited descriptors, and those it opens (receipt,
/// signalfd, pidfds, std's spawn error pipe) are close-on-exec already.
/// Marking instead of closing in the child keeps std's error pipe working.
fn close_inherited_on_exec() -> io::Result<()> {
    // SAFETY: integer-only syscall (Linux 5.11+ for the flag).
    let marked = unsafe {
        libc::syscall(
            libc::SYS_close_range,
            3 as libc::c_uint,
            libc::c_uint::MAX,
            libc::CLOSE_RANGE_CLOEXEC,
        )
    };
    if marked == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if !matches!(error.raw_os_error(), Some(libc::ENOSYS | libc::EINVAL)) {
        return Err(error);
    }
    // Older kernels: walk the descriptor table instead.
    let listed: Vec<RawFd> = std::fs::read_dir("/proc/self/fd")?
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
        .collect();
    for fd in listed.into_iter().filter(|fd| *fd > 2) {
        // SAFETY: fcntl on a descriptor number; the listing's own descriptor
        // is closed by now and simply reports EBADF.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags >= 0 && unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Raise the guard's soft descriptor limit to its hard limit, as headroom for
/// pidfds. Returns the original limit, which the browser gets back, or None
/// when nothing changed. Best effort: a refusal keeps the original limit.
fn raise_descriptor_limit() -> Option<libc::rlimit64> {
    let mut original = libc::rlimit64 {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: valid rlimit pointers.
    if unsafe { libc::getrlimit64(libc::RLIMIT_NOFILE, &mut original) } != 0
        || original.rlim_cur >= original.rlim_max
    {
        return None;
    }
    let raised = libc::rlimit64 {
        rlim_cur: original.rlim_max,
        rlim_max: original.rlim_max,
    };
    (unsafe { libc::setrlimit64(libc::RLIMIT_NOFILE, &raised) } == 0).then_some(original)
}

/// The browser: the guard's stdio (the App's control pipe and log),
/// environment and process group, no blocked signals, and the guard's
/// original descriptor limit.
fn browser_command(
    program: &OsString,
    args: &[OsString],
    limit: Option<libc::rlimit64>,
) -> Command {
    let mut command = Command::new(program);
    command.args(args);
    // SAFETY: runs in the forked child before exec and makes only system
    // calls (sigprocmask, prlimit64), which are async-signal-safe.
    unsafe {
        command.pre_exec(move || {
            // A signal mask survives fork and exec, and std's Command leaves
            // it as is (observed with Rust 1.97). Start the browser with none
            // blocked, whatever the guard or the App's thread had blocked.
            let mut none: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut none);
            if libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
            if let Some(limit) = limit {
                let restored = libc::syscall(
                    libc::SYS_prlimit64,
                    0,
                    libc::RLIMIT_NOFILE,
                    &limit as *const libc::rlimit64,
                    std::ptr::null_mut::<libc::rlimit64>(),
                );
                if restored != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    command
}

struct Prepared {
    receipt: File,
    signals: Signals,
    stopping: bool,
    limit: Option<libc::rlimit64>,
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
    if let Err(error) = probe() {
        return Err((
            2,
            format!("pidfd or /proc children are unavailable: {error}"),
        ));
    }
    if let Err(error) = close_inherited_on_exec() {
        return Err((
            2,
            format!("Inherited descriptors cannot be withheld: {error}"),
        ));
    }
    // Reserve the receipt before any browser can exist. O_EXCL refuses a path
    // or link created meanwhile; the explicit mode survives any umask.
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(receipt)
        .map_err(|error| (2, format!("The receipt cannot be created: {error}")))?;
    let fail = |error: io::Error| (1, format!("Cannot guard the browser lifecycle: {error}"));
    file.set_permissions(Permissions::from_mode(0o600))
        .map_err(fail)?;
    // Blocked before the parent-death signal is armed, so that signal and any
    // stop request are queued for the signalfd instead of ending the guard.
    let signals = Signals::block().map_err(fail)?;
    let limit = raise_descriptor_limit();
    // SAFETY: getppid has no preconditions; prctl takes integer arguments only.
    let parent = unsafe { libc::getppid() };
    let one: libc::c_ulong = 1;
    let term = libc::SIGTERM as libc::c_ulong;
    let zero: libc::c_ulong = 0;
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, one, zero, zero, zero) } != 0
        || unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, term, zero, zero, zero) } != 0
    {
        return Err(fail(io::Error::last_os_error()));
    }
    // The parent-death signal only covers deaths after it is armed. A parent
    // that ended between reading it and arming the signal has already
    // reparented this guard, so the pid differs: treat that as a stop now.
    // (The App also arms it in the forked guard before exec.)
    let stopping = unsafe { libc::getppid() } != parent;
    Ok(Prepared {
        receipt: file,
        signals,
        stopping,
        limit,
    })
}

// STOP_OBSERVATION_BEGIN
const STOP_OBSERVATION_CAP: usize = 64;

#[derive(Clone, Copy)]
struct StopEvent {
    kind: &'static str,
    pid: i32,
    start_tick: Option<u64>,
    root: bool,
    target: Option<&'static str>,
    signal: Option<&'static str>,
    result: Option<&'static str>,
    errno: Option<i32>,
    status: Option<i32>,
    readiness: Option<PidfdReadiness>,
    comm_class: Option<CommClass>,
}

struct StopObservation {
    active: bool,
    records: [Option<StopEvent>; STOP_OBSERVATION_CAP],
    used: usize,
    truncated: bool,
}

impl Default for StopObservation {
    fn default() -> Self {
        Self {
            active: false,
            records: [None; STOP_OBSERVATION_CAP],
            used: 0,
            truncated: false,
        }
    }
}

impl StopObservation {
    fn begin(&mut self) {
        self.active = true;
    }

    fn push(&mut self, event: StopEvent) {
        if !self.active {
            return;
        }
        if self.used < STOP_OBSERVATION_CAP {
            self.records[self.used] = Some(event);
            self.used += 1;
        } else {
            self.truncated = true;
        }
    }

    fn signal(
        &mut self,
        pid: i32,
        start_tick: Option<u64>,
        root: bool,
        target: &'static str,
        signal: libc::c_int,
        sent: &io::Result<()>,
        readiness: Option<PidfdReadiness>,
    ) {
        self.signal_with_comm(pid, start_tick, root, target, signal, sent, readiness, None);
    }

    fn signal_with_comm(
        &mut self,
        pid: i32,
        start_tick: Option<u64>,
        root: bool,
        target: &'static str,
        signal: libc::c_int,
        sent: &io::Result<()>,
        readiness: Option<PidfdReadiness>,
        comm_class: Option<CommClass>,
    ) {
        let errno = sent.as_ref().err().and_then(io::Error::raw_os_error);
        let result = if sent.is_ok() {
            "ok"
        } else if errno == Some(libc::ESRCH) {
            "esrch"
        } else {
            "error"
        };
        let signal = if signal == libc::SIGTERM {
            "term"
        } else if signal == libc::SIGKILL {
            "kill"
        } else {
            "other"
        };
        self.push(StopEvent {
            kind: "signal",
            pid,
            start_tick,
            root,
            target: Some(target),
            signal: Some(signal),
            result: Some(result),
            errno,
            status: None,
            readiness,
            comm_class,
        });
    }

    fn reaped(&mut self, pid: i32, root: bool, status: i32) {
        // A waitpid PID cannot safely be rejoined to an old pidfd generation.
        self.push(StopEvent {
            kind: "reap",
            pid,
            start_tick: None,
            root,
            target: None,
            signal: None,
            result: None,
            errno: None,
            status: Some(status),
            readiness: None,
            comm_class: None,
        });
    }

    fn json(&self) -> serde_json::Value {
        let records: Vec<_> = self.records[..self.used].iter().flatten().map(|event| {
            serde_json::json!({ "kind": event.kind, "pid": event.pid, "startTick": event.start_tick,
                "root": event.root, "target": event.target, "signal": event.signal, "result": event.result,
                "errno": event.errno, "waitStatus": event.status,
                "commClass": event.comm_class.map(|class| class as u8),
                "readiness": event.readiness.map(|ready| serde_json::json!([ready.polled, ready.revents, ready.terminal, ready.poll_errno])) })
        }).collect();
        serde_json::json!({ "schema": "takosumi.guard-stop-observation@v1", "diagnosticOnly": true,
            "scope": "existing_drain", "readinessFields": ["polled", "revents", "terminal", "pollErrno"],
            "commClasses": COMM_CLASSES, "commClassScope": "matching_birth_stat_snapshot",
            "capacity": STOP_OBSERVATION_CAP, "truncated": self.truncated, "records": records })
    }
}
// STOP_OBSERVATION_END

// LAUNCH_CONTEXT_BEGIN
// Private Android fixture metadata. It never supplies signal eligibility,
// ownership, drain budgets or a normal-exit verdict. No reads occur in drain.
const LAUNCH_CONTEXT_ENV: &str = "TAKOSUMI_GUARD_LAUNCH_CONTEXT";
const LAUNCH_CONTEXT_CAP: usize = 16;
const LAUNCH_ARG_BYTES: usize = 8192;

#[derive(Clone, Debug, PartialEq)]
struct LaunchPoint {
    start: u64,
    parent: i32,
    parent_start: u64,
    image: [u64; 2],
    argv: Vec<u8>, // Transient only: never serialized or logged.
}

fn launch_image(path: impl AsRef<Path>) -> Option<(File, [u64; 2])> {
    use std::os::unix::fs::MetadataExt;
    // O_PATH pins the image inode without reading executable contents. The
    // identity is not a binary digest, SDK version or proof of purpose.
    let file = OpenOptions::new().read(true).custom_flags(libc::O_PATH).open(path).ok()?;
    let meta = file.metadata().ok()?;
    meta.is_file().then_some((file, [meta.dev(), meta.ino()]))
}

fn launch_argv(bytes: &[u8]) -> Option<Vec<&[u8]>> {
    if bytes.is_empty() || bytes.len() > LAUNCH_ARG_BYTES || !bytes.ends_with(&[0]) {
        return None;
    }
    let args: Vec<_> = bytes[..bytes.len() - 1].split(|b| *b == 0).collect();
    (args.len() <= 64 && !args[0].is_empty()).then_some(args)
}

fn launch_options<'a>(args: &'a [&'a [u8]]) -> Option<(&'a [u8], &'a [u8])> {
    let option = |name: &[u8]| {
        let hits: Vec<_> = args.iter().enumerate().filter(|(_, arg)| **arg == name).collect();
        if hits.len() != 1 { return None; }
        args.get(hits[0].0 + 1).copied().filter(|v| !v.is_empty() && !v.starts_with(b"-"))
    };
    Some((option(b"-avd")?, option(b"-port")?))
}

// ARGUMENT_SHAPE_BEGIN
// Syntax hints from the already accepted historical point. They do not attest
// an SDK parser/role, actual dispatch, exec-at-signal or signal necessity.
fn launch_argument_shape(args: Option<&[&[u8]]>, declared_exact: Option<bool>, parent: Option<i32>)
    -> (Option<&'static str>, Option<i32>, Option<&'static str>)
{
    let (Some(args), Some(parent)) = (args, parent.filter(|pid| *pid > 0)) else {
        return (None, None, None);
    };
    let positive = |value: &[u8]| -> Option<i32> {
        if value.is_empty() || value.len() > 10 || !matches!(value[0], b'1'..=b'9')
            || !value.iter().all(u8::is_ascii_digit) { return None; }
        std::str::from_utf8(value).ok()?.parse::<i32>().ok().filter(|n| *n > 0)
    };
    if args.len() == 5 && args[1] == b"-kill" && args[3] == b"-sleep" {
        if let (Some(target), Some(wait)) = (positive(args[2]), positive(args[4])) {
            return (Some("canonical_kill_sleep"), Some(wait), Some(if target == parent {
                "same_retained_parent_pid"
            } else { "different_pid" }));
        }
    }
    (Some(if declared_exact == Some(true) { "declared_exact" } else { "other_complete" }), None, None)
}
// ARGUMENT_SHAPE_END

// Parent lifetime must be the guard's actual birth or a retained owned tuple.
// Missing parents, reparenting, PID reuse or a moving image/argv refuse a point.
fn launch_point(pid: i32, start: u64, owner: i32, owner_start: Option<u64>,
                tracker: &Tracker) -> Option<(LaunchPoint, File)> {
    use std::io::Read;
    let before = ancestry(pid).ok()??;
    if before.0 != start { return None; }
    let parent_before = ancestry(before.1).ok()??;
    let parent_start = if before.1 == owner {
        owner_start?
    } else {
        tracker.handles.get(&before.1)?.0
    };
    if parent_before.0 != parent_start { return None; }
    let (image_file, image) = launch_image(format!("/proc/{pid}/exe"))?;
    let mut argv = Vec::new();
    File::open(format!("/proc/{pid}/cmdline")).ok()?
        .take((LAUNCH_ARG_BYTES + 1) as u64).read_to_end(&mut argv).ok()?;
    launch_argv(&argv)?;
    if ancestry(pid).ok()?? != before || ancestry(before.1).ok()?? != parent_before {
        return None;
    }
    Some((LaunchPoint { start, parent: before.1, parent_start, image, argv }, image_file))
}

// A point is accepted only if both full samples agree. This is bookending,
// not an atomic snapshot: same-image exec or changes between reads may be lost.
// CHANGE_HISTORY_BEGIN
// Closed component bits: image, argv, parent PID, parent birth, expected child
// birth (within a pair only), and an unclassified difference fallback.
fn launch_change_components(a: &LaunchPoint, b: &LaunchPoint) -> u8 {
    u8::from(a.image != b.image) | (u8::from(a.argv != b.argv) << 1)
        | (u8::from(a.parent != b.parent) << 2) | (u8::from(a.parent_start != b.parent_start) << 3)
}

#[derive(Clone, Copy)]
struct LaunchChange {
    first_attempt: u16,
    within: u8,
    across: u8,
}
// CHANGE_HISTORY_END

fn coherent_launch_points(start: u64, first: Option<&LaunchPoint>, last: Option<&LaunchPoint>)
    -> Result<Option<LaunchPoint>, u8>
{
    match (first, last) {
        (Some(a), Some(b)) if a.start == start && a == b => Ok(Some(a.clone())),
        (Some(a), Some(b)) => {
            let components = launch_change_components(a, b)
                | if a.start != start || b.start != start { 16 } else { 0 };
            Err(if components == 0 { 32 } else { components })
        }
        _ => Ok(None),
    }
}

struct LaunchRecord {
    observation: u16,
    last_attempt: u16,
    last_attempt_unknown: bool,
    point: Option<LaunchPoint>,
    changed: bool,
    change: Option<LaunchChange>,
}

impl LaunchRecord {
    fn changed_at(&mut self, observation: u16, within: u8, across: u8) {
        self.changed = true;
        let change = self.change.get_or_insert(LaunchChange { first_attempt: observation, within: 0, across: 0 });
        change.within |= within;
        change.across |= across;
    }

    fn sample(&mut self, observation: u16, sampled: Result<Option<LaunchPoint>, u8>) {
        self.last_attempt = observation;
        self.last_attempt_unknown = true;
        match sampled {
            Err(components) => self.changed_at(observation, if components == 0 { 32 } else { components }, 0),
            Ok(None) => {} // Missing later reads do not erase a dated historical snapshot.
            Ok(Some(point)) => {
                if let Some(old) = &self.point {
                    if old.image != point.image || old.argv != point.argv
                        || old.parent != point.parent || old.parent_start != point.parent_start {
                        let components = launch_change_components(old, &point);
                        self.changed_at(observation, 0, components);
                    }
                }
                if !self.changed { self.point = Some(point); self.observation = observation;
                    self.last_attempt_unknown = false; }
            }
        }
    }
}

struct LaunchContext {
    guard: i32,
    guard_start: Option<u64>,
    // Keep the requested inode pinned through final serialization.
    image: Option<(File, [u64; 2])>,
    argv: Option<Vec<u8>>,
    records: BTreeMap<(i32, u64), LaunchRecord>,
    observation: u16,
    cursor: usize,
    truncated: bool,
}

impl LaunchContext {
    fn enabled(value: Option<&std::ffi::OsStr>) -> bool {
        value == Some(std::ffi::OsStr::new("android_fixture_sdk"))
    }

    fn new(guard: i32, program: &OsString, args: &[OsString]) -> Self {
        use std::os::unix::ffi::OsStrExt;
        let mut argv = Vec::new();
        for arg in std::iter::once(program).chain(args) {
            if argv.len().saturating_add(arg.as_bytes().len() + 1) > LAUNCH_ARG_BYTES {
                argv.clear();
                break;
            }
            argv.extend_from_slice(arg.as_bytes());
            argv.push(0);
        }
        let argv = launch_argv(&argv).is_some().then_some(argv);
        Self { guard, guard_start: start_time(guard).ok().flatten(), image: launch_image(program),
            argv, records: BTreeMap::new(), observation: 0, cursor: 0, truncated: false }
    }

    // At most one owned identity per running tick; never called by send/drain.
    // IO/scheduling overhead can delay running observation and is not claimed zero.
    fn capture(&mut self, tracker: &Tracker) {
        if tracker.handles.is_empty() { return; }
        let Some(next) = self.observation.checked_add(1) else {
            self.truncated = true;
            return;
        };
        self.observation = next;
        let fresh = tracker.handles.iter().find(|(pid, (start, _, _))|
            !self.records.contains_key(&(**pid, *start)));
        let selected = fresh.or_else(|| tracker.handles.iter().nth(self.cursor % tracker.handles.len()));
        self.cursor = self.cursor.wrapping_add(1);
        let Some((&pid, &(start, _, _))) = selected else { return; };
        if !self.records.contains_key(&(pid, start)) && self.records.len() >= LAUNCH_CONTEXT_CAP {
            self.truncated = true;
            // Retain the latest bounded history instead of permanently losing
            // every later SDK child after the first sixteen lifetimes.
            if let Some((&oldest, _)) = self.records.iter().min_by_key(|(_, r)| r.last_attempt) {
                self.records.remove(&oldest);
            }
        }
        // Hold both executable handles until comparison, preventing inode reuse
        // between the samples from masquerading as equal image identities.
        let first = launch_point(pid, start, self.guard, self.guard_start, tracker);
        let last = launch_point(pid, start, self.guard, self.guard_start, tracker);
        let sampled = coherent_launch_points(start, first.as_ref().map(|v| &v.0), last.as_ref().map(|v| &v.0));
        let record = self.records.entry((pid, start)).or_insert(LaunchRecord {
            observation: next, last_attempt: next, last_attempt_unknown: true, point: None, changed: false,
            change: None,
        });
        record.sample(next, sampled);
    }

    fn json(&self, root: Option<i32>) -> serde_json::Value {
        let declared_image = self.image.as_ref().map(|v| v.1);
        let declared_args = self.argv.as_deref().and_then(launch_argv);
        let declared_options = declared_args.as_deref().and_then(launch_options);
        let records: Vec<_> = self.records.iter().map(|(&(pid, start), record)| {
            let point = record.point.as_ref().filter(|_| !record.changed);
            let args = point.and_then(|p| launch_argv(&p.argv));
            let options = args.as_deref().and_then(launch_options);
            let declared_exact = point.and_then(|p| self.argv.as_ref().map(|argv| p.argv == *argv));
            let shape = launch_argument_shape(args.as_deref(), declared_exact, point.map(|p| p.parent));
            let mut row = serde_json::json!({ "pid": pid, "startTick": start, "root": root == Some(pid),
                "parentPid": point.map(|p| p.parent), "parentStartTick": point.map(|p| p.parent_start),
                "observation": record.observation, "lastAttempt": record.last_attempt,
                "lastAttemptUnknown": record.last_attempt_unknown,
                "status": if record.changed { "changed" } else if point.is_some() { "observed" } else { "unknown" },
                "execIdentity": point.map(|p| p.image),
                "matchesDeclaredImage": point.and_then(|p| declared_image.map(|image| p.image == image)),
                "matchesDeclaredArgv": declared_exact,
                "matchesDeclaredRunOptions": options.zip(declared_options).map(|(a, b)| a == b),
                "dispatchArgKind": shape.0, "helperWaitSeconds": shape.1, "killTargetRelation": shape.2,
            });
            // Only a retained accepted point can supply the strictly older hint.
            // These are observation ordinals, not event times or current roles.
            if let Some(change) = record.change.as_ref().filter(|_| record.changed) {
                let historical = record.point.as_ref().filter(|p| p.start == start && p.parent > 0
                    && record.observation < change.first_attempt
                    && (p.parent != self.guard || self.guard_start == Some(p.parent_start)))
                    .and_then(|p| {
                        let args = launch_argv(&p.argv)?;
                        let shape = launch_argument_shape(Some(&args), self.argv.as_ref().map(|argv| p.argv == *argv), Some(p.parent));
                        Some(serde_json::json!([record.observation, p.parent, p.parent_start, shape.0?, shape.1, shape.2]))
                    });
                row["change"] = serde_json::json!([change.first_attempt, change.within, change.across]);
                row["historicalArg"] = historical.unwrap_or(serde_json::Value::Null);
            }
            row
        }).collect();
        serde_json::json!({ "schema": "takosumi.guard-launch-context@v1", "source": "owned_running_bookends",
            "guardPid": self.guard, "guardStartTick": self.guard_start, "spawnedRootPid": root,
            "declaredLaunch": { "imageIdentity": declared_image, "runOptionsPresent": declared_options.is_some() },
            "capacity": LAUNCH_CONTEXT_CAP, "truncated": self.truncated,
            "absenceIsUnknown": true, "signalTimeStateUnknown": true, "records": records })
    }
}
// LAUNCH_CONTEXT_END

struct Run {
    me: i32,
    tracker: Tracker,
    child: Option<i32>,
    /// Taken right after spawn; the direct child is signalled only through it.
    child_fd: Option<OwnedFd>,
    root_exit: Option<i32>,
    forced: BTreeSet<i32>,
    cleanup_errors: BTreeSet<String>,
    non_fatal: BTreeSet<String>,
    stop_observation: StopObservation,
    launch_context: Option<LaunchContext>, // Private opt-in; no cleanup authority.
}

fn exit_code(status: libc::c_int) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        -libc::WTERMSIG(status)
    } else {
        status
    }
}

impl Run {
    /// Reap every exited child. `Ok(true)` only on ECHILD: the kernel's proof
    /// that this guard has no child left, live or zombie. An empty list of
    /// handles is not proof. `__WALL` also covers children that report with a
    /// signal other than SIGCHLD, so ECHILD cannot hide one of those.
    fn reap(&mut self) -> Result<bool, String> {
        loop {
            let mut status = 0;
            // SAFETY: a valid status pointer; WNOHANG never blocks.
            let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG | libc::__WALL) };
            if pid == 0 {
                return Ok(false); // The kernel still has at least one live child.
            }
            if pid < 0 {
                let error = io::Error::last_os_error();
                match error.raw_os_error() {
                    Some(libc::ECHILD) => return Ok(true),
                    Some(libc::EINTR) => continue,
                    _ => return Err(fault(&error)),
                }
            }
            self.stop_observation.reaped(
                pid,
                Some(pid) == self.child && self.root_exit.is_none(),
                status,
            );
            if Some(pid) == self.child && self.root_exit.is_none() {
                self.root_exit = Some(exit_code(status));
            }
        }
    }

    fn safe_observe(&mut self) {
        if let Err(error) = self.tracker.observe(self.me) {
            self.cleanup_errors.insert(error);
        }
    }

    fn safe_reap(&mut self) -> bool {
        self.reap().unwrap_or_else(|error| {
            self.cleanup_errors.insert(error);
            false
        })
    }

    /// Signal every retained descendant through its pidfd, and the direct
    /// child while it is unreaped.
    fn send(&mut self, signal: libc::c_int) {
        self.safe_observe();
        for (pid, (start_tick, fd, class)) in &self.tracker.handles {
            let readiness = signal_readiness(signal, Some(fd), pidfd_readiness);
            if readiness.is_some_and(|ready| ready.terminal) {
                continue;
            }
            let sent = pidfd_signal(fd, signal);
            self.stop_observation.signal_with_comm(
                *pid,
                Some(*start_tick),
                Some(*pid) == self.child && self.root_exit.is_none(),
                "retained_pidfd",
                signal,
                &sent,
                readiness,
                Some(*class),
            );
            match sent {
                Ok(()) => {
                    self.forced.insert(*pid);
                }
                Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {}
                Err(error) => {
                    self.cleanup_errors.insert(fault(&error));
                }
            }
        }
        // Covers an observation failure before the direct child was retained.
        if let (Some(pid), None) = (self.child, self.root_exit) {
            if self.child_fd.is_none() {
                self.child_fd = child_pidfd(pid).ok(); // Retry a failed open.
            }
            let readiness = signal_readiness(signal, self.child_fd.as_ref(), pidfd_readiness);
            if readiness.is_some_and(|ready| ready.terminal) {
                return;
            }
            let sent = match &self.child_fd {
                Some(fd) => pidfd_signal(fd, signal),
                // Still no pidfd: signal by pid. SIGCHLD has its default
                // disposition and this guard has not reaped the child
                // (root_exit unset), so the kernel keeps the pid for it.
                // SAFETY: kill takes integers only.
                None => match unsafe { libc::kill(pid, signal) } {
                    0 => Ok(()),
                    _ => Err(io::Error::last_os_error()),
                },
            };
            self.stop_observation.signal(
                pid,
                None,
                true,
                if self.child_fd.is_some() {
                    "direct_pidfd"
                } else {
                    "direct_pid"
                },
                signal,
                &sent,
                readiness,
            );
            match sent {
                Ok(()) => {}
                Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {}
                Err(error) => {
                    self.cleanup_errors.insert(fault(&error));
                }
            }
        }
    }

    /// Wait for the browser to exit or for a stop request, without a lifetime
    /// cap. Returns whether the browser exited before a stop was requested.
    fn running(&mut self, signals: &Signals) -> Result<bool, String> {
        let mut stopping = false;
        let mut faults = 0;
        while self.root_exit.is_none() && !stopping {
            // A failed observation must not end a healthy browser: the drain
            // observes again and ECHILD decides completeness.
            if let Err(error) = self.tracker.observe(self.me) {
                self.non_fatal.insert(error);
            }
            if let Some(context) = &mut self.launch_context {
                context.capture(&self.tracker);
            }
            // Neither may a failed reap or wait, unless it keeps failing: then
            // the guard can no longer see the browser end or a stop request,
            // and it drains after MAX_RUNNING_FAULTS consecutive ticks.
            let mut failed = None;
            let reaped = if injected("REAP") {
                Err(fault(&injected_error()))
            } else {
                self.reap()
            };
            match reaped {
                Ok(true) if self.root_exit.is_none() => {
                    // No child left, yet this guard never reaped the browser:
                    // its exit status is unknown.
                    return Err("RootExitLost".to_string());
                }
                Ok(_) => {}
                Err(error) => failed = Some(error),
            }
            if self.root_exit.is_none() {
                let waited = if injected("WAIT") {
                    Err(injected_error())
                } else {
                    signals.wait(RUNNING_TICK)
                };
                match waited {
                    Ok(stop) => stopping |= stop,
                    Err(error) => {
                        failed = Some(fault(&error));
                        std::thread::sleep(RUNNING_TICK);
                    }
                }
            }
            match failed {
                None => faults = 0,
                Some(error) => {
                    self.non_fatal.insert(error.clone());
                    faults += 1;
                    if faults >= MAX_RUNNING_FAULTS {
                        return Err(error);
                    }
                }
            }
        }
        Ok(!stopping)
    }

    /// The bounded drain every run takes, however it ended. Returns whether
    /// the kernel reported no children (ECHILD).
    fn drain(&mut self, signals: &Signals, clean_exit_grace: Option<Duration>) -> bool {
        self.stop_observation.begin();
        'phases: for (signal, budget) in DRAIN {
            if let Some(signal) = signal {
                self.send(signal);
            }
            let start = Instant::now();
            let grace = if signal.is_none() { clean_exit_grace } else { None };
            let mut deadline = start + grace.unwrap_or(budget);
            loop {
                self.safe_observe();
                if self.safe_reap() {
                    break 'phases;
                }
                if let Some(signal) = signal {
                    self.send(signal); // Includes newly adopted late-fork helpers.
                }
                if grace.is_some() && !self.cleanup_errors.is_empty() {
                    deadline = deadline.min(start + budget);
                }
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                // A child exit wakes this early. Explicit stop requests keep
                // the standard drain even after a normal SDK exit.
                let pause = DRAIN_TICK.min(deadline - now);
                match signals.wait(pause) {
                    Ok(true) if grace.is_some() => deadline = deadline.min(start + budget),
                    Ok(_) => {}
                    Err(_) => {
                        if grace.is_some() {
                            deadline = deadline.min(start + budget);
                        }
                        std::thread::sleep(pause);
                    }
                }
            }
        }
        // No child can appear after ECHILD: every living descendant has an
        // ancestor that is a direct child of this guard, or was adopted by
        // this subreaper and is itself a direct child. With no child there is
        // no descendant left to fork, and the guard starts nothing new.
        self.safe_reap()
    }
}

fn supervise(
    prepared: Prepared,
    program: &OsString,
    args: &[OsString],
    clean_exit_grace: Option<Duration>,
) -> ExitCode {
    let Prepared {
        mut receipt,
        signals,
        stopping,
        limit,
    } = prepared;
    let mut run = Run {
        // SAFETY: getpid has no preconditions.
        me: unsafe { libc::getpid() },
        tracker: Tracker::default(),
        child: None,
        child_fd: None,
        root_exit: None,
        forced: BTreeSet::new(),
        cleanup_errors: BTreeSet::new(),
        non_fatal: BTreeSet::new(),
        stop_observation: StopObservation::default(),
        launch_context: None,
    };
    if LaunchContext::enabled(std::env::var_os(LAUNCH_CONTEXT_ENV).as_deref()) {
        run.launch_context = Some(LaunchContext::new(run.me, program, args));
    }
    let mut failure = None;
    let mut exited_without_stop = false;
    // A stop requested before the browser starts (a parent already gone, or a
    // pending SIGTERM or SIGINT) starts nothing.
    match signals.wait(Duration::ZERO) {
        Err(error) => failure = Some(fault(&error)),
        Ok(pending) if pending || stopping => {
            failure = Some("StopRequestedBeforeStart".to_string());
        }
        Ok(_) => match browser_command(program, args, limit).spawn() {
            Ok(child) => {
                // Reaped only by `Run::reap`; std's Child neither waits nor
                // kills on drop. SIGCHLD has its default disposition, so the
                // unreaped child keeps its pid and this pidfd names it.
                let pid = child.id() as i32;
                run.child = Some(pid);
                match child_pidfd(pid) {
                    Ok(fd) => run.child_fd = Some(fd),
                    // Not fatal: `send` retries and can signal by pid.
                    Err(error) => {
                        run.non_fatal.insert(fault(&error));
                    }
                }
                match run.running(&signals) {
                    Ok(natural) => exited_without_stop = natural,
                    Err(error) => failure = Some(error),
                }
            }
            Err(error) => failure = Some(fault(&error)),
        },
    }
    let grace = clean_exit_grace
        .filter(|_| exited_without_stop && run.root_exit == Some(0) && run.non_fatal.is_empty());
    let empty = run.drain(&signals, grace);
    let complete = run.child.is_some() && run.root_exit.is_some() && empty;
    let normal =
        complete && run.root_exit == Some(0) && failure.is_none() && run.cleanup_errors.is_empty();
    let mut proof = serde_json::json!({
        "rootExit": run.root_exit,
        "complete": complete,
        "remaining": run.tracker.alive(),
        "kernelNoChildren": empty,
        "observed": run.tracker.observed,
        "forcedCleanup": run.forced.len(),
        "error": failure,
        "cleanupErrors": run.cleanup_errors,
        "nonFatalErrors": run.non_fatal,
        "stopObservation": run.stop_observation.json(),
    });
    if let Some(context) = &run.launch_context {
        proof["launchContext"] = context.json(run.child);
    }
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
#[path = "guard_tests.rs"]
mod tests;
