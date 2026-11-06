//! Windows process ownership for the per-profile browser path (Windows only).
//!
//! A Job Object that ends every process in it when its last handle closes, a
//! way to start a process inside one before it runs any code, and the named
//! event an App uses to ask a browser guard to stop. The App's adapter owns
//! one job for all of its guards (Windows has no parent-death signal: when
//! the App ends, its job ends every guard and browser), and
//! `takosumi-browser-guard` owns one job per browser (every process the
//! browser starts is in it, so the guard can prove that none is left). Jobs
//! nest (Windows 8 and later): a browser is in its guard's job and, through
//! it, in the App's.
//!
//! Minimal `extern "system"` declarations of documented kernel32 functions;
//! no crate dependency. Struct layouts are checked for x86_64.

use std::{
    ffi::{c_void, OsStr},
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle},
        process::CommandExt,
    },
    process::{Child, Command},
    time::Duration,
};

/// The environment variable that names a guard's stop event. The App sets
/// it for the guard; the guard removes it from the browser's environment.
pub const STOP_EVENT_ENV: &str = "TAKOSUMI_BROWSER_GUARD_STOP_EVENT";
/// `CreateProcess` flag: a console program gets a console without a window
/// (and so a windowless console host, `conhost.exe`, as its child). The guard
/// starts the browser this way: CEF's helper processes, console programs
/// too, then share that one hidden console instead of each getting a new one.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// `CreateProcess` flag: a console program gets no console at all, and so no
/// console host. The App starts the guard this way; its standard handles
/// are the pipes and log file passed explicitly.
pub const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_SUSPENDED: u32 = 0x0000_0004;
const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
const JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION: i32 = 1;
const JOB_OBJECT_BASIC_PROCESS_ID_LIST: i32 = 3;
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
const TH32CS_SNAPTHREAD: u32 = 0x4;
const THREAD_SUSPEND_RESUME: u32 = 0x2;
const SYNCHRONIZE: u32 = 0x0010_0000;
const ERROR_ALREADY_EXISTS: i32 = 183;
const ERROR_MORE_DATA: i32 = 234;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;
const INFINITE: u32 = u32::MAX;
const INVALID_HANDLE_VALUE: isize = -1;
/// Process ids read from a job at once; a longer list is reported partly.
const MAX_LISTED: usize = 1024;

type Handle = *mut c_void;

#[repr(C)]
#[derive(Default)]
struct BasicLimit {
    // JOBOBJECT_BASIC_LIMIT_INFORMATION
    per_process_user_time_limit: i64,
    per_job_user_time_limit: i64,
    limit_flags: u32,
    minimum_working_set_size: usize,
    maximum_working_set_size: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
}
#[repr(C)]
#[derive(Default)]
struct ExtendedLimit {
    // JOBOBJECT_EXTENDED_LIMIT_INFORMATION
    basic: BasicLimit,
    io: [u64; 6],
    process_memory_limit: usize,
    job_memory_limit: usize,
    peak_process_memory_used: usize,
    peak_job_memory_used: usize,
}
#[repr(C)]
#[derive(Default)]
struct Accounting {
    // JOBOBJECT_BASIC_ACCOUNTING_INFORMATION
    total_user_time: i64,
    total_kernel_time: i64,
    this_period_total_user_time: i64,
    this_period_total_kernel_time: i64,
    total_page_fault_count: u32,
    total_processes: u32,
    active_processes: u32,
    total_terminated_processes: u32,
}
#[repr(C)]
struct ProcessIdList {
    // JOBOBJECT_BASIC_PROCESS_ID_LIST with room for MAX_LISTED ids
    assigned: u32,
    listed: u32,
    ids: [usize; MAX_LISTED],
}
#[repr(C)]
#[derive(Default)]
struct ThreadEntry {
    // THREADENTRY32
    size: u32,
    usage: u32,
    thread: u32,
    owner: u32,
    base_priority: i32,
    delta_priority: i32,
    flags: u32,
}
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(std::mem::size_of::<BasicLimit>() == 64);
    assert!(std::mem::size_of::<ExtendedLimit>() == 144);
    assert!(std::mem::size_of::<Accounting>() == 48);
    assert!(std::mem::size_of::<ThreadEntry>() == 28);
    assert!(std::mem::size_of::<ProcessIdList>() == 8 + 8 * MAX_LISTED);
};

#[link(name = "kernel32")]
extern "system" {
    fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> Handle;
    fn SetInformationJobObject(job: Handle, class: i32, info: *mut c_void, length: u32) -> i32;
    fn QueryInformationJobObject(
        job: Handle,
        class: i32,
        info: *mut c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
    fn TerminateJobObject(job: Handle, exit_code: u32) -> i32;
    fn IsProcessInJob(process: Handle, job: Handle, result: *mut i32) -> i32;
    fn GetCurrentProcess() -> Handle;
    fn CreateToolhelp32Snapshot(flags: u32, process: u32) -> Handle;
    fn Thread32First(snapshot: Handle, entry: *mut ThreadEntry) -> i32;
    fn Thread32Next(snapshot: Handle, entry: *mut ThreadEntry) -> i32;
    fn OpenThread(access: u32, inherit: i32, thread: u32) -> Handle;
    fn ResumeThread(thread: Handle) -> u32;
    fn CreateEventW(attributes: *mut c_void, manual: i32, initial: i32, name: *const u16)
        -> Handle;
    fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> Handle;
    fn SetEvent(event: Handle) -> i32;
    fn SetLastError(code: u32);
    fn WaitForMultipleObjects(count: u32, handles: *const Handle, all: i32, millis: u32) -> u32;
}

/// A handle a creating function returned, or its error: null (and for
/// toolhelp, INVALID_HANDLE_VALUE) means failure.
fn owned(handle: Handle) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle as isize == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a fresh handle this process now owns alone.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle as RawHandle) })
}

fn check(result: i32) -> io::Result<()> {
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(Some(0)).collect()
}

/// A Job Object whose processes all end when its last handle closes. Its
/// processes cannot break away from it, and the handle is not inheritable.
pub struct Job(OwnedHandle);

impl Job {
    pub fn kill_on_close() -> io::Result<Self> {
        // SAFETY: null attributes and name create a new unnamed job.
        let job = Self(owned(unsafe {
            CreateJobObjectW(std::ptr::null_mut(), std::ptr::null())
        })?);
        let mut limits = ExtendedLimit::default();
        limits.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: a live job and a correctly sized information block.
        check(unsafe {
            SetInformationJobObject(
                job.raw(),
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                (&mut limits as *mut ExtendedLimit).cast(),
                std::mem::size_of::<ExtendedLimit>() as u32,
            )
        })?;
        Ok(job)
    }

    fn raw(&self) -> Handle {
        self.0.as_raw_handle().cast()
    }

    pub fn assign(&self, child: &Child) -> io::Result<()> {
        // SAFETY: a live job and the live, unreaped child's process handle.
        check(unsafe { AssignProcessToJobObject(self.raw(), child.as_raw_handle().cast()) })
    }

    /// (processes ever in the job, processes in it now), nested jobs included.
    pub fn processes(&self) -> io::Result<(u32, u32)> {
        let mut info = Accounting::default();
        // SAFETY: a live job and a correctly sized information block.
        check(unsafe {
            QueryInformationJobObject(
                self.raw(),
                JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION,
                (&mut info as *mut Accounting).cast(),
                std::mem::size_of::<Accounting>() as u32,
                std::ptr::null_mut(),
            )
        })?;
        Ok((info.total_processes, info.active_processes))
    }

    /// The ids of the processes in the job now (at most `MAX_LISTED`).
    pub fn process_ids(&self) -> io::Result<Vec<u32>> {
        let mut list = Box::new(ProcessIdList {
            assigned: 0,
            listed: 0,
            ids: [0; MAX_LISTED],
        });
        // SAFETY: a live job and a correctly sized information block.
        let queried = check(unsafe {
            QueryInformationJobObject(
                self.raw(),
                JOB_OBJECT_BASIC_PROCESS_ID_LIST,
                (&mut *list as *mut ProcessIdList).cast(),
                std::mem::size_of::<ProcessIdList>() as u32,
                std::ptr::null_mut(),
            )
        });
        match queried {
            Ok(()) => {}
            // More processes than fit: the list holds the first ones.
            Err(error) if error.raw_os_error() == Some(ERROR_MORE_DATA) => {}
            Err(error) => return Err(error),
        }
        let listed = (list.listed as usize).min(MAX_LISTED);
        Ok(list.ids[..listed].iter().map(|id| *id as u32).collect())
    }

    /// End every process in the job now, each with `exit_code`.
    pub fn terminate(&self, exit_code: u32) -> io::Result<()> {
        // SAFETY: a live job.
        check(unsafe { TerminateJobObject(self.raw(), exit_code) })
    }
}

/// Start `command` inside `job` before it runs any code: it is created
/// suspended (with `flags`), assigned, then resumed. One that cannot be
/// assigned or resumed is killed and reaped, never left running outside the
/// job. This replaces any creation flags set on `command`.
pub fn spawn_in(job: &Job, command: &mut Command, flags: u32) -> io::Result<Child> {
    command.creation_flags(flags | CREATE_SUSPENDED);
    let mut child = command.spawn()?;
    if let Err(error) = job.assign(&child).and_then(|()| resume(child.id())) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    Ok(child)
}

/// Resume the threads of a process created suspended (its initial thread).
/// The caller holds the unreaped child, so its id names that process.
fn resume(pid: u32) -> io::Result<()> {
    // SAFETY: a snapshot of every thread in the system.
    let snapshot = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) })?;
    let snapshot: Handle = snapshot.as_raw_handle().cast();
    let mut entry = ThreadEntry {
        size: std::mem::size_of::<ThreadEntry>() as u32,
        ..Default::default()
    };
    let mut resumed = 0;
    // SAFETY: a live snapshot and an entry with its size set.
    let mut found = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while found {
        if entry.owner == pid {
            // SAFETY: a thread id from the snapshot; the result is checked.
            let thread = owned(unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.thread) })?;
            // SAFETY: a live thread handle with suspend/resume access.
            if unsafe { ResumeThread(thread.as_raw_handle().cast()) } == u32::MAX {
                return Err(io::Error::last_os_error());
            }
            resumed += 1;
        }
        entry.size = std::mem::size_of::<ThreadEntry>() as u32;
        // SAFETY: as above.
        found = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    if resumed == 0 {
        return Err(io::Error::other(
            "The suspended process had no thread to resume.",
        ));
    }
    Ok(())
}

/// Whether this process belongs to any Job Object.
pub fn in_job() -> io::Result<bool> {
    let mut result = 0;
    // SAFETY: the current-process pseudo handle and a valid result pointer;
    // a null job asks about any job.
    check(unsafe { IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut result) })?;
    Ok(result != 0)
}

/// A manual-reset event: the App creates it under a fresh name and keeps it
/// for the guard's lifetime; the guard opens it by that name to wait on it.
/// Like a signal on Linux, any process of the same user could set it.
pub struct Event(OwnedHandle);

impl Event {
    /// A new event; a name that already exists is refused, never reused.
    pub fn create(name: &str) -> io::Result<Self> {
        let name = wide(OsStr::new(name));
        // SAFETY: plain calls; null attributes and a NUL-terminated name.
        // The error code is cleared first: only ERROR_ALREADY_EXISTS after
        // a successful call means the name existed.
        let event = owned(unsafe {
            SetLastError(0);
            CreateEventW(std::ptr::null_mut(), 1, 0, name.as_ptr())
        })?;
        if io::Error::last_os_error().raw_os_error() == Some(ERROR_ALREADY_EXISTS) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "The guard stop event name is already in use.",
            ));
        }
        Ok(Self(event))
    }

    /// An existing event, for waiting only.
    pub fn open(name: &OsStr) -> io::Result<Self> {
        let name = wide(name);
        // SAFETY: a NUL-terminated name; the result is checked.
        Ok(Self(owned(unsafe {
            OpenEventW(SYNCHRONIZE, 0, name.as_ptr())
        })?))
    }

    pub fn set(&self) -> io::Result<()> {
        // SAFETY: a live event handle.
        check(unsafe { SetEvent(self.0.as_raw_handle().cast()) })
    }

    pub fn is_set(&self) -> io::Result<bool> {
        Ok(wait_any(&[self.as_raw_handle()], Some(Duration::ZERO))?.is_some())
    }
}

impl AsRawHandle for Event {
    fn as_raw_handle(&self) -> RawHandle {
        self.0.as_raw_handle()
    }
}

/// Wait until one of `handles` is signalled: its index, or None when the
/// timeout passed first (`None` waits without a timeout).
pub fn wait_any(handles: &[RawHandle], timeout: Option<Duration>) -> io::Result<Option<usize>> {
    let millis = timeout.map_or(INFINITE, |timeout| {
        timeout.as_millis().min(u128::from(INFINITE - 1)) as u32
    });
    let raw: Vec<Handle> = handles.iter().map(|handle| handle.cast()).collect();
    // SAFETY: live handles owned by the caller for the whole call.
    let result = unsafe { WaitForMultipleObjects(raw.len() as u32, raw.as_ptr(), 0, millis) };
    if result == WAIT_TIMEOUT {
        return Ok(None);
    }
    let index = result.wrapping_sub(WAIT_OBJECT_0) as usize;
    if index < raw.len() {
        Ok(Some(index))
    } else {
        Err(io::Error::last_os_error())
    }
}
