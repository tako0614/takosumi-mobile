//! Product supervisor for one per-profile CEF browser process (macOS, M2).
//!
//! `takosumi-browser-guard <absolute new receipt> <program> [args...]`
//!
//! The same command line, receipt fields and exit codes as the Linux guard
//! (`guard_main.rs`) and the Windows guard (`guard_windows.rs`). macOS has
//! neither Linux's child subreaper and pidfds nor Windows' Job Objects, and
//! kqueue's `NOTE_TRACK` (following forks) is no longer supported, so the
//! guard owns the browser's processes by these means:
//!
//! - **Process group.** The App starts the guard as the leader of its own
//!   process group (it refuses otherwise); the browser and every process it
//!   starts inherit that group unless they leave it. Every member of the
//!   group other than the guard is the browser's (`proc_listpgrppids`; a
//!   listing that does not hold the guard itself is a failure, since libproc
//!   answers an error with an empty list).
//! - **Descendants, recorded while they run.** A descendant that leaves the
//!   group (`setsid`, `setpgid`) is still found as a child of a process the
//!   guard already tracks (`proc_listchildpids`). The browser's orphans are
//!   reparented to launchd, so the guard records every descendant
//!   continuously while the browser runs: on every kqueue event, at least
//!   every 250 ms, and at once whenever a tracked process forks (`NOTE_FORK`
//!   on each tracked process). It keeps tracking each one afterwards by pid
//!   and start time, and lists a tracked process's children only while
//!   that pid still has the recorded start time.
//! - **PID reuse.** Each tracked process is kept with its start time
//!   (`proc_pidinfo`, `PROC_PIDTBSDINFO`); one whose start time differs, or
//!   that started before the parent it was found under, is not the browser's
//!   and is never signalled. A zombie counts as ended.
//!
//! The guard waits on one kqueue, without a lifetime cap: the browser's exit
//! (`EVFILT_PROC`, `NOTE_EXIT`), a stop request (`SIGTERM` or `SIGINT`,
//! `EVFILT_SIGNAL`, as the App sends `SIGTERM` on Linux), and the App's death
//! (`NOTE_EXIT` on the guard's parent, the App: macOS has no parent-death
//! signal; a parent already gone at start is a stop request before the
//! browser starts, and a changed parent at any later look is the App's death
//! too). Both signals are blocked from the guard's start until kqueue
//! watches them; one already pending then is a stop request. They are then
//! ignored in the guard, which kqueue still records, and restored to their
//! defaults in the browser.
//!
//! The drain then always runs: up to 0.5 s for the browser
//! and every tracked process to end by themselves. After an observed clean
//! browser exit, allow 1.5 s instead: Chromium's clone-cleanup helper polls
//! for its parent's exit once a second before deleting its temporary clone.
//! This grants no process an exception or ownership based on its role.
//! Then the guard freezes
//! the tree before it kills anything: `SIGSTOP` to the unreaped browser, to
//! each tracked process still the same (start time) and to every other
//! running member of the group, each verified right before its signal; it
//! looks again for processes they started before they stopped and stops
//! those too, until a look taken after every stopped process showed stopped
//! finds nothing new and no stopped process has reported a fork since (a
//! fork or spawn a thread was already making may complete after the stop,
//! as XNU's code suggests; not verified), for at most 0.5 s. A stopped
//! process should not fork or exit by itself, and its children stay listed
//! under it, so the kill is meant to find the whole tree. Then `SIGKILL` to
//! every stopped process, leaves first: each level of the stopped tree is
//! given up to 100 ms to be gone before its parents are killed, so no
//! stopped child is orphaned while it still runs (a process group orphaned
//! with stopped members gets `SIGHUP` and `SIGCONT`). A stopped process
//! keeps its pid, so one whose state cannot be read is killed too (the
//! error recorded). For the rest of a 2 s window (counted from the freeze)
//! every `DRAIN_TICK` the guard kills whatever it still finds the same way;
//! at its end every stopped process still there (or unreadable) is killed
//! again and makes the run incomplete, and so does a fork reported by a
//! stopped process after the freeze's last look (`ForkAfterFreeze` in
//! `cleanupErrors`). From the first `SIGSTOP` to that point a drop guard
//! kills every stopped process should the drain return or unwind early, and
//! nothing in it may panic: the one diagnostic line (written after the
//! kill, to standard error) ignores a failed write. Forced cleanup counts,
//! for the browser and every other process alike, the processes the
//! guard's `SIGKILL` reached while they ran (verified, not a zombie). For a
//! process the freeze saw stopped that is all but certain (it could only
//! have ended meanwhile by another's signal); for one it did not see
//! stopped (the freeze ran out, or it was found only after the kill), one
//! that ended by itself at that moment is counted too: macOS gives the
//! guard no exit status for a process that is not its child, so the count
//! errs only on the safe side.
//!
//! **What completion means here.** `kernelNoChildren` and `complete` are, on
//! macOS, "no survivor among the processes the guard observed": after the
//! browser was reaped, the group lists only the guard and no tracked process
//! is alive. That is not a kernel proof as on Linux (the subreaper's
//! `waitid` answering `ECHILD`) or Windows (the job's accounting). Outside it
//! are a process the browser started through a system service (launchd,
//! XPC), which is not its descendant (as on Linux one started by a session
//! manager is not), and a double fork faster than an observation: a process
//! that leaves the group and whose parent ends before the guard's next look
//! (the `NOTE_FORK` wake, or the next tick) is orphaned to launchd unseen.
//! M1 observed ordinary CEF helpers in the group. Chromium's code-sign-clone
//! cleanup helper can leave it and relies on the descendant tracking above.
//!
//! Receipt, written once after the drain, with the other guards' fields:
//! `{rootExit, complete, remaining, kernelNoChildren, observed, forcedCleanup,
//! error, cleanupErrors, nonFatalErrors}`. `remaining` lists the process ids
//! still in the group or tracked; `observed` counts every process seen, by
//! pid and start time (the browser included, once). `rootExit` is the browser's exit
//! code, or minus the signal that ended it. The guard exits 0 only when the
//! drain found no survivor, the browser exited 0 and nothing failed; a
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

#[path = "mac_cleanup_role.rs"]
mod cleanup_role;

/// Opt-in for native diagnostic fixtures only. Default runs never read argv.
const ROLE_DIAGNOSTICS: &str = "TAKOSUMI_MACOS_GUARD_ROLE_DIAGNOSTICS";

/// Best-effort closed role from a bounded, complete KERN_PROCARGS2 response.
/// XNU can return a suffix on a short successful read: query size before and
/// after, require it below the cap, and account for the query's 4-byte round-up.
/// Even then XNU permits stale data across same-PID execve. Never authority.
fn read_cleanup_role(pid: i32) -> Option<&'static str> {
    const CAP: usize = 64 * 1024;
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let size = |mib: &mut [i32; 3]| -> Option<usize> {
        let mut count = 0;
        // SAFETY: three valid integer selectors, no output or new value.
        let result = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                std::ptr::null_mut(),
                &mut count,
                std::ptr::null_mut(),
                0,
            )
        };
        (result == 0).then_some(count)
    };
    let expected = size(&mut mib)?;
    if !(5..CAP).contains(&expected) {
        return None;
    }
    let mut bytes = vec![0u8; CAP];
    let mut count = bytes.len();
    // SAFETY: the writable output allocation has `count` bytes; no new value.
    let result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            bytes.as_mut_ptr().cast(),
            &mut count,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0
        || count >= CAP
        || count.next_multiple_of(4) != expected
        || size(&mut mib) != Some(expected)
    {
        return None;
    }
    cleanup_role::parse(&bytes[..count])
}

struct RoleObservation {
    pid: i32,
    start: u64,
    role: Option<&'static str>,
    identities_unchanged: bool,
    elapsed_micros: u128,
}

/// While the browser runs, observe its processes on every event and at
/// least this often.
const RUNNING_TICK: Duration = Duration::from_millis(250);
/// While draining, re-observe at least this often.
const DRAIN_TICK: Duration = Duration::from_millis(20);
/// Natural end, then the freeze and the kill, and this long (from the
/// freeze) for everything to be gone.
const NATURAL: Duration = Duration::from_millis(500);
/// Chromium 154's ChromeCodeSignCloneCleanupMain polls getppid() once per
/// second before deleting the code-sign clone. After a clean root exit,
/// allow that poll plus the existing 500 ms scheduling/deletion margin.
/// Stops/App death already observed before drain, nonzero exits and recorded
/// observation failures keep NATURAL. The phase is fixed at drain entry;
/// later events never restart/extend it. No role diagnostic selects a deadline.
const CLEAN_EXIT_NATURAL: Duration = Duration::from_millis(1500);
const FORCED: Duration = Duration::from_secs(2);
/// The longest the freeze waits for its stopped processes to show stopped,
/// within the forced window.
const FREEZE: Duration = Duration::from_millis(500);
/// The longest the kill waits for one level of the stopped tree to be gone
/// before it kills their parents.
const LEVEL: Duration = Duration::from_millis(100);
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
            note(format_args!("{message}"));
            ExitCode::from(code)
        }
    }
}

/// One line on standard error (the profile's browser.log). A failed write
/// (a full disk, a closed log) is ignored: `eprintln!` would panic, and the
/// guard must never stop half way through its drain.
fn note(line: std::fmt::Arguments) {
    let _ = writeln!(io::stderr().lock(), "takosumi-browser-guard: {line}");
}

/// A short, stable error name for the receipt; never a path or argv (the
/// guard's own errors carry a fixed name).
fn fault(error: &io::Error) -> String {
    match (error.raw_os_error(), error.kind()) {
        (Some(code), kind) => format!("{kind:?} (errno {code})"),
        (None, io::ErrorKind::Other) => error.to_string(),
        (None, kind) => format!("{kind:?}"),
    }
}

/// What the kernel says about one process: its parent, group, start time
/// (microseconds since the epoch), whether it is a zombie and whether it is
/// stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Facts {
    ppid: i32,
    pgid: i32,
    start: u64,
    zombie: bool,
    stopped: bool,
    /// Kernel's bounded process name, never its arguments or environment.
    name: [u8; 32],
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
            // Saturating: no arithmetic here may panic (it runs inside the
            // drain's stop-to-kill section too).
            start: info
                .pbi_start_tvsec
                .saturating_mul(1_000_000)
                .saturating_add(info.pbi_start_tvusec),
            zombie: info.pbi_status == libc::SZOMB,
            stopped: info.pbi_status == libc::SSTOP,
            name: info.pbi_name.map(|byte| byte as u8),
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

/// libproc's listing calls answer a failed system call with 0 (and its
/// errno), not -1: errno is cleared before each call and an empty answer
/// with errno set is the failure.
fn list(call: Listing, id: i32) -> io::Result<Vec<i32>> {
    let mut room = LISTING;
    loop {
        let mut pids = vec![0 as libc::pid_t; room];
        let bytes = (room * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: errno is this thread's; the buffer holds `room` pids and
        // its size is passed.
        let count = unsafe {
            *libc::__error() = 0;
            call(id, pids.as_mut_ptr().cast(), bytes)
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        if count == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error().is_some_and(|code| code != 0) {
                return Err(error);
            }
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

/// The members of the guard's own group: a listing without the guard itself
/// (which leads the group and runs) is incomplete and fails.
fn own_group(me: i32) -> io::Result<Vec<i32>> {
    let members = group(me)?;
    if members.contains(&me) {
        Ok(members)
    } else {
        Err(io::Error::other("GroupListingWithoutTheGuard"))
    }
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

/// One kqueue: the browser's exit, the App's exit, stop signals and the
/// forks of tracked processes.
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

    /// The browser: its exit and its forks.
    fn watch_browser(&self, pid: i32) -> io::Result<()> {
        self.change(
            pid as usize,
            libc::EVFILT_PROC,
            libc::NOTE_EXIT | libc::NOTE_FORK,
        )
    }

    /// Any other tracked process: its forks, so a new child is looked for at
    /// once instead of at the next tick.
    fn watch_forks(&self, pid: i32) -> io::Result<()> {
        self.change(pid as usize, libc::EVFILT_PROC, libc::NOTE_FORK)
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
        // Any event (a fork included) makes the caller look at the tree;
        // only an exit of the browser or the App ends the wait.
        let mut woke = Woke::default();
        let count = (count as usize).min(events.len());
        for event in &events[..count] {
            let exited = event.fflags & libc::NOTE_EXIT != 0;
            match event.filter {
                libc::EVFILT_SIGNAL => woke.stop = true,
                libc::EVFILT_PROC if Some(event.ident as i32) == browser => {
                    woke.browser_exited |= exited
                }
                libc::EVFILT_PROC if event.ident as i32 == parent => woke.stop |= exited,
                _ => {}
            }
        }
        Ok(woke)
    }

    /// Every process that reported a fork since the queue was last read,
    /// without waiting (other events are dropped: only the drain calls it).
    fn forks(&self) -> io::Result<BTreeSet<i32>> {
        let zero = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let mut forked = BTreeSet::new();
        for _ in 0..64 {
            // SAFETY: kevent is plain data; the buffer's length is passed.
            let mut events: [libc::kevent; 8] = unsafe { std::mem::zeroed() };
            let count = unsafe {
                libc::kevent(
                    self.0.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    events.as_mut_ptr(),
                    events.len() as libc::c_int,
                    &zero,
                )
            };
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return Err(error);
            }
            let count = (count as usize).min(events.len());
            for event in &events[..count] {
                if event.filter == libc::EVFILT_PROC && event.fflags & libc::NOTE_FORK != 0 {
                    forked.insert(event.ident as i32);
                }
            }
            if count < events.len() {
                break;
            }
        }
        Ok(forked)
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

/// The stop signals as a set.
fn stop_signals() -> libc::sigset_t {
    // SAFETY: sigset_t is plain data, filled by sigemptyset/sigaddset.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGINT);
        set
    }
}

/// Block, unblock (`SIG_BLOCK`, `SIG_UNBLOCK`) the stop signals.
fn mask_stops(how: libc::c_int) -> io::Result<()> {
    let set = stop_signals();
    // SAFETY: a valid set; the old mask is not wanted.
    if unsafe { libc::sigprocmask(how, &set, std::ptr::null_mut()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Whether a stop signal is pending (sent while blocked).
fn stop_pending() -> io::Result<bool> {
    // SAFETY: sigpending fills a plain set.
    unsafe {
        let mut pending: libc::sigset_t = std::mem::zeroed();
        if libc::sigpending(&mut pending) != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(libc::sigismember(&pending, libc::SIGTERM) == 1
            || libc::sigismember(&pending, libc::SIGINT) == 1)
    }
}

fn prepare(args: &[OsString]) -> Result<Prepared, (u8, String)> {
    let refuse = |message: &str| Err((2, message.to_string()));
    // A stop request from now on is kept pending until kqueue watches for
    // it (the App also starts the guard with both blocked).
    if let Err(error) = mask_stops(libc::SIG_BLOCK) {
        return Err((2, format!("The stop signals cannot be blocked: {error}")));
    }
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
    // Stop requests: kqueue records every one sent from now on, blocked or
    // ignored; one sent before is pending. Then they are ignored here
    // (which discards the pending ones, already counted) and unblocked;
    // the browser gets them back at their defaults. SIGCHLD keeps its
    // default, so the browser's exit status is not discarded.
    for signal in [libc::SIGTERM, libc::SIGINT] {
        queue.watch_signal(signal).map_err(fail)?;
    }
    let mut stopping = stop_pending().map_err(fail)?;
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
    mask_stops(libc::SIG_UNBLOCK).map_err(fail)?;
    // The App's death: its exit, watched from now on. A parent already gone
    // (this guard was reparented to launchd) is a stop before the start.
    // SAFETY: getppid has no preconditions.
    let parent = unsafe { libc::getppid() };
    stopping |= parent == 1
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
    /// A stop signal or changed App parent seen while supervising. A zero
    /// browser exit in that same event batch is still a stop, not normal quit.
    stop_observed: bool,
    /// Processes of the browser's still running, with their start times.
    tracked: BTreeMap<i32, u64>,
    /// Every process seen, by pid and start time (the browser included),
    /// retaining its first parent edge before a later orphan becomes PPID 1.
    observed: BTreeMap<(i32, u64), FirstObserved>,
    forced: u32,
    cleanup_errors: BTreeSet<String>,
    non_fatal: BTreeSet<String>,
    role_diagnostics: bool,
}

struct FirstObserved {
    parent: i32,
    /// The parent's already-tracked birth, if available; no extra OS read.
    tracked_parent_start: Option<u64>,
    group: i32,
}

/// A process the freeze stopped: its start time, its parent (for the kill
/// order) and whether it has shown stopped since.
struct Frozen {
    start: u64,
    ppid: i32,
    stopped: bool,
    pgid: i32,
    name: [u8; 32],
}

impl Run {
    /// One owned, orphaned process in its own group, before any SIGSTOP.
    /// No raw arguments survive the read. This observation cannot affect
    /// ownership, signals, forced count or receipt acceptance. A syscall has
    /// no hard latency bound: opt-in runs are timing-perturbed diagnostics.
    fn diagnose_cleanup_role(&self) -> Option<RoleObservation> {
        if !self.role_diagnostics {
            return None;
        }
        for (&pid, &start) in self.tracked.iter().take(32) {
            let Some(before) = facts(pid).ok().flatten() else {
                continue;
            };
            if before.start != start || before.zombie || before.ppid != 1 || before.pgid != pid {
                continue;
            }
            let reading = Instant::now();
            let role = read_cleanup_role(pid);
            let unchanged = facts(pid).ok().flatten() == Some(before);
            return Some(RoleObservation {
                pid,
                start,
                role: role.filter(|_| unchanged),
                identities_unchanged: unchanged,
                elapsed_micros: reading.elapsed().as_micros(),
            });
        }
        None
    }

    fn remember(&mut self, pid: i32, facts: Facts) {
        let tracked_parent_start = self
            .tracked
            .get(&facts.ppid)
            .copied()
            .filter(|start| *start <= facts.start);
        self.observed
            .entry((pid, facts.start))
            .or_insert(FirstObserved {
                parent: facts.ppid,
                tracked_parent_start,
                group: facts.pgid,
            });
    }

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
        if pid == self.me || facts.zombie {
            return;
        }
        self.remember(pid, facts);
        if self.tracked.contains_key(&pid) {
            return;
        }
        self.tracked.insert(pid, facts.start);
        // Its forks wake the guard at once (the browser's own watch, with
        // its exit, is set when it starts).
        if Some(pid) != self.child {
            match self.queue.watch_forks(pid) {
                Ok(()) => {}
                Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {}
                Err(error) => {
                    self.non_fatal.insert(fault(&error));
                }
            }
        }
    }

    /// Record the browser's processes now: every member of the group, and
    /// the children of every tracked process; drop those that ended or
    /// whose pid now names another process. Returns the first failure.
    fn observe(&mut self) -> Result<(), String> {
        let mut problem = None;
        match own_group(self.me) {
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
            // The parent must still be the process recorded: a reused pid
            // would pull in another process's children.
            match facts(parent) {
                Ok(Some(now)) if now.start == parent_start && !now.zombie => {}
                Ok(_) => continue,
                Err(error) => {
                    problem = Some(fault(&error));
                    continue;
                }
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

    /// The group's members other than the guard: those that run, and those
    /// whose state cannot be read (they may still run).
    fn group_left(&mut self) -> Option<Vec<i32>> {
        match own_group(self.me) {
            Ok(mut members) => {
                members.retain(|pid| *pid != self.me);
                // A zombie whose parent is gone is reaped by launchd; it no
                // longer runs.
                members.retain(|pid| match facts(*pid) {
                    Ok(Some(facts)) => !facts.zombie,
                    Ok(None) => false,
                    Err(_) => true,
                });
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
    /// death), observing on every event (a tracked process's fork included)
    /// and every tick.
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
                    // A changed parent is the App's death as well, should
                    // its exit event be missed.
                    // SAFETY: getppid has no preconditions.
                    let orphaned = unsafe { libc::getppid() } != self.parent;
                    self.stop_observed |= woke.stop || orphaned;
                    if self.stop_observed || woke.browser_exited || self.root_exit.is_some() {
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

    /// The processes to end now: the unreaped browser (always), every
    /// tracked process and every member of the group.
    fn candidates(&mut self) -> BTreeSet<i32> {
        let mut pids: BTreeSet<i32> = self.tracked.keys().copied().collect();
        pids.extend(self.group_left().unwrap_or_default());
        if let (Some(child), None) = (self.child, self.root_exit) {
            pids.insert(child);
        }
        pids.remove(&self.me);
        pids
    }

    /// Whether `pid` is, right now, a running process of the browser's, and
    /// what the kernel says about it: the unreaped browser (its pid cannot
    /// be reused while unreaped), a tracked process with its recorded start
    /// time, or a member of the guard's group; never a zombie.
    fn verify(&mut self, pid: i32) -> Option<Facts> {
        if pid == self.me {
            return None;
        }
        let now = match facts(pid) {
            Ok(Some(now)) if !now.zombie => now,
            Ok(_) => return None,
            Err(error) => {
                self.cleanup_errors.insert(fault(&error));
                return None;
            }
        };
        let browser = Some(pid) == self.child && self.root_exit.is_none();
        let same = self.tracked.get(&pid) == Some(&now.start);
        (browser || same || now.pgid == self.me).then_some(now)
    }

    /// Send `signal`; a process that ended meanwhile (`ESRCH`) is no error.
    fn signal(&mut self, pid: i32, signal: libc::c_int) -> bool {
        // SAFETY: kill with a plain pid and signal, verified by the caller.
        if unsafe { libc::kill(pid, signal) } == 0 {
            return true;
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            self.cleanup_errors.insert(fault(&error));
        }
        false
    }

    /// Stop every process of the browser's, then look again for any it
    /// started before it stopped, until a look taken after every stopped
    /// process showed stopped finds nothing new and no stopped process
    /// reported a fork since (a fork a thread was already making can still
    /// complete after the stop), or `until`. Every stopped process enters
    /// `freezer` as soon as its stop succeeds.
    fn freeze(&mut self, freezer: &mut Freezer, until: Instant) {
        // Forks reported before the freeze are looked at by its first look.
        if let Err(error) = self.queue.forks() {
            self.cleanup_errors.insert(fault(&error));
        }
        loop {
            // Which stopped processes show stopped, before this look.
            let mut all_stopped = true;
            let mut ended = Vec::new();
            for (pid, entry) in freezer
                .frozen
                .iter_mut()
                .filter(|(_, entry)| !entry.stopped)
            {
                match facts(*pid) {
                    Ok(Some(now)) if now.start == entry.start && !now.zombie => {
                        entry.stopped = now.stopped;
                        all_stopped &= now.stopped;
                    }
                    // It ended after (or as) it was stopped, not by the kill.
                    Ok(_) => ended.push(*pid),
                    Err(error) => {
                        self.cleanup_errors.insert(fault(&error));
                        all_stopped = false;
                    }
                }
            }
            for pid in ended {
                freezer.frozen.remove(&pid);
            }
            self.reap();
            if let Err(error) = self.observe() {
                self.cleanup_errors.insert(error);
            }
            let mut found = false;
            for pid in self.candidates() {
                if freezer.frozen.contains_key(&pid) {
                    continue;
                }
                let Some(now) = self.verify(pid) else {
                    continue;
                };
                if self.signal(pid, libc::SIGSTOP) {
                    self.remember(pid, now);
                    freezer.frozen.insert(
                        pid,
                        Frozen {
                            start: now.start,
                            ppid: now.ppid,
                            stopped: false,
                            pgid: now.pgid,
                            name: now.name,
                        },
                    );
                    found = true;
                }
            }
            let mut quiet = !found && all_stopped;
            if quiet {
                // A fork reported by a stopped process since the last read
                // means another look.
                match self.queue.forks() {
                    Ok(forked) => {
                        quiet = !forked.iter().any(|pid| freezer.frozen.contains_key(pid));
                    }
                    Err(error) => {
                        self.cleanup_errors.insert(fault(&error));
                    }
                }
            }
            if quiet || Instant::now() >= until {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// SIGKILL every stopped process, leaves first: each level of the
    /// stopped tree (by parent) is killed and given up to `LEVEL` (and never
    /// past `deadline`) to be gone before its parents are, so no stopped
    /// child is orphaned while it still runs (a process group orphaned with
    /// stopped members gets SIGHUP and SIGCONT). A stopped process keeps its
    /// pid, so one that cannot be read is killed too (its error recorded).
    /// Returns the kills that reached a running process.
    fn kill_frozen(&mut self, freezer: &Freezer, deadline: Instant) -> BTreeSet<(i32, u64)> {
        let depth = |pid: i32| {
            let mut depth = 0usize;
            let mut at = pid;
            while let Some(entry) = freezer.frozen.get(&at) {
                if depth > freezer.frozen.len() || entry.ppid == at {
                    break;
                }
                depth += 1;
                at = entry.ppid;
            }
            depth
        };
        let mut levels: BTreeMap<usize, Vec<(i32, u64)>> = BTreeMap::new();
        for (pid, entry) in &freezer.frozen {
            levels
                .entry(depth(*pid))
                .or_default()
                .push((*pid, entry.start));
        }
        let mut killed = BTreeSet::new();
        for (_, level) in levels.into_iter().rev() {
            for &(pid, start) in &level {
                match facts(pid) {
                    Ok(Some(now)) if now.start == start && !now.zombie => {}
                    // Gone meanwhile: ended by someone else.
                    Ok(_) => continue,
                    Err(error) => {
                        self.cleanup_errors.insert(fault(&error));
                    }
                }
                if self.signal(pid, libc::SIGKILL) {
                    killed.insert((pid, start));
                }
            }
            let level_end = (Instant::now() + LEVEL).min(deadline);
            loop {
                self.reap();
                let gone = level.iter().all(|&(pid, start)| match facts(pid) {
                    Ok(None) => true,
                    Ok(Some(now)) => now.zombie || now.start != start,
                    Err(_) => false,
                });
                if gone || Instant::now() >= level_end {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        killed
    }

    /// The bounded drain every run takes. Returns whether it found no
    /// survivor among the observed processes.
    fn drain(&mut self, natural: Duration) -> bool {
        if self.settle_until(Instant::now() + natural) {
            return true;
        }
        let freezing = Instant::now();
        let deadline = freezing + FORCED;
        // Opt-in diagnostic IO is before SIGSTOP and charged to the existing
        // deadline. It never extends the natural or forced cleanup windows.
        let role_observation = self.diagnose_cleanup_role();
        // From the first SIGSTOP until the kill, nothing may leave the
        // stopped processes behind: the freezer kills them if this section
        // returns early or unwinds.
        let mut freezer = Freezer {
            frozen: BTreeMap::new(),
            armed: true,
        };
        self.freeze(&mut freezer, (freezing + FREEZE).min(deadline));
        let freeze_ms = freezing.elapsed().as_millis();
        let mut killed = self.kill_frozen(&freezer, deadline);
        // For the rest of the window, whatever is still found (one that
        // escaped the freeze) is killed the same way.
        let mut empty = loop {
            if self.settled() {
                break true;
            }
            let now = Instant::now();
            if now >= deadline {
                break false;
            }
            for pid in self.candidates() {
                let Some(found) = self.verify(pid) else {
                    continue;
                };
                if killed.contains(&(pid, found.start)) {
                    continue;
                }
                if self.signal(pid, libc::SIGKILL) {
                    self.remember(pid, found);
                    killed.insert((pid, found.start));
                }
            }
            std::thread::sleep(DRAIN_TICK.min(deadline - now));
        };
        // Every stopped process must be gone by now. One still the same, or
        // one that cannot be read, is killed again and the run is not
        // complete (macOS hands out pids in sequence, so within this window
        // a killed pid is not anyone else's yet).
        for (&pid, entry) in &freezer.frozen {
            let alive = match facts(pid) {
                Ok(Some(now)) => now.start == entry.start && !now.zombie,
                Ok(None) => false,
                Err(error) => {
                    self.cleanup_errors.insert(fault(&error));
                    true
                }
            };
            if alive {
                empty = false;
                if self.signal(pid, libc::SIGKILL) {
                    killed.insert((pid, entry.start));
                }
            }
        }
        // A stopped process that reported a fork after the freeze's last
        // look may have a child nobody saw: not complete.
        match self.queue.forks() {
            Ok(forked) if forked.iter().any(|pid| freezer.frozen.contains_key(pid)) => {
                self.cleanup_errors.insert("ForkAfterFreeze".to_string());
                empty = false;
            }
            Ok(_) => {}
            Err(error) => {
                self.cleanup_errors.insert(fault(&error));
                empty = false;
            }
        }
        freezer.armed = false;
        self.forced = self
            .forced
            .saturating_add(u32::try_from(killed.len()).unwrap_or(u32::MAX));
        // One diagnostic line (browser.log), after the kill: how many
        // processes the freeze stopped, how many it saw stopped, and how
        // long the freeze took.
        note(format_args!(
            "freeze {{\"stopped\":{},\"shownStopped\":{},\"freezeMs\":{}}}",
            freezer.frozen.len(),
            freezer
                .frozen
                .values()
                .filter(|entry| entry.stopped)
                .count(),
            freeze_ms
        ));
        if let Some(observed) = role_observation {
            note(format_args!(
                "cleanup-role {}",
                serde_json::json!({"pid":observed.pid,"start":observed.start,
                    "role":observed.role,"identitiesUnchanged":observed.identities_unchanged,
                    "elapsedMicros":observed.elapsed_micros,"diagnosticOnly":true})
            ));
        }
        // Diagnose only after every stopped process has been killed and
        // the drop guard disarmed. Neither IO nor serialization can delay
        // the stop-to-kill section. Bound the log, and never emit argv.
        for &(pid, start) in killed.iter().take(32) {
            let frozen = freezer
                .frozen
                .get(&pid)
                .filter(|entry| entry.start == start);
            let snapshot = frozen.map(|entry| {
                let end = entry
                    .name
                    .iter()
                    .position(|byte| *byte == 0)
                    .unwrap_or(entry.name.len());
                serde_json::json!({
                    "parent": entry.ppid,
                    "group": entry.pgid,
                    "name": String::from_utf8_lossy(&entry.name[..end]),
                    "shownStopped": entry.stopped,
                })
            });
            note(format_args!(
                "forced {}",
                serde_json::json!({
                    "pid": pid,
                    "start": start,
                    // null means this identity was not retained by the freeze:
                    // it was found later or ended during the stop observation.
                    "freezeSnapshot": snapshot,
                    "firstObserved": self.observed.get(&(pid, start)).map(|first| serde_json::json!({
                        "parent": first.parent,
                        "trackedParentStart": first.tracked_parent_start,
                        "group": first.group,
                    })),
                })
            ));
        }
        if killed.len() > 32 {
            note(format_args!("forced omitted={}", killed.len() - 32));
        }
        empty
    }
}

/// Every process the freeze stopped. While armed, dropping it (an early
/// return or a panic between the first SIGSTOP and the end of the kill)
/// SIGKILLs each one that is still the same process or cannot be read: a
/// stopped process keeps its pid until it is killed.
struct Freezer {
    frozen: BTreeMap<i32, Frozen>,
    armed: bool,
}

impl Drop for Freezer {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        for (&pid, entry) in &self.frozen {
            let same = match facts(pid) {
                Ok(Some(now)) => now.start == entry.start && !now.zombie,
                Ok(None) => false,
                Err(_) => true,
            };
            if same {
                // SAFETY: kill with a plain pid; identity checked above.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
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
        stop_observed: stopping,
        tracked: BTreeMap::new(),
        observed: BTreeMap::new(),
        forced: 0,
        cleanup_errors: BTreeSet::new(),
        non_fatal: BTreeSet::new(),
        role_diagnostics: std::env::var(ROLE_DIAGNOSTICS).as_deref() == Ok("1"),
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
                // A failed read records nothing here: the first look records
                // the browser, and the receipt counts it once in any case.
                if let Ok(Some(facts)) = facts(pid) {
                    run.tracked.insert(pid, facts.start);
                    run.remember(pid, facts);
                }
                match run.queue.watch_browser(pid) {
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
    let natural = if run.root_exit == Some(0)
        && !run.stop_observed
        && failure.is_none()
        && run.non_fatal.is_empty()
        && run.cleanup_errors.is_empty()
    {
        CLEAN_EXIT_NATURAL
    } else {
        NATURAL
    };
    let empty = run.drain(natural);
    let complete = run.child.is_some() && run.root_exit.is_some() && empty;
    // The browser counts once, also when no look ever read its start time.
    let mut observed = run.observed.len();
    if let Some(child) = run.child {
        if !run.observed.keys().any(|(pid, _)| *pid == child) {
            observed += 1;
        }
    }
    let mut remaining: BTreeSet<i32> = run.tracked.keys().copied().collect();
    remaining.extend(run.group_left().unwrap_or_default());
    let normal =
        complete && run.root_exit == Some(0) && failure.is_none() && run.cleanup_errors.is_empty();
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
        note(format_args!("the receipt could not be written: {error}"));
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
