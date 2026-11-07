#!/usr/bin/env python3
"""Smoke test of takosumi-cef-browser-process on its own (no guard, no App).

Written for Windows (the windows-latest CI job) and kept OS neutral, so it also
runs against the Linux browser and, since M1, the macOS one (the executable
inside its app bundle, `<bundle>.app/Contents/MacOS/takosumi-cef-browser-process`,
scripts/bundle-macos-cef.mjs). Python 3 standard library only (ctypes for the
Windows Job Object and window checks, and on macOS for `sandbox_check`); on
macOS it also reads the process table with `ps`.

It creates a new owned case directory and lays it out as the App's adapter
(src-tauri/plugins/app-screen/src/desktop/process_adapter.rs) lays out a
browser's profile directory: `boot.json` holding the first Open request,
`Downloads/`, and on Linux private HOME/XDG/TMPDIR directories. It starts the
browser directly with `TAKOSUMI_BROWSER_PROCESS_DIRECTORY` and the App's
switches: on Windows suspended, inside a kill-on-close Job Object, resumed
once assigned, so every process it starts is accounted; on Linux and macOS in
its own session and process group (macOS has no Job Objects: before the
shutdown it also records the browser's descendants by parent pid, and each
must have ended afterwards, since an orphan is reparented to launchd). On
macOS the browser also gets `--use-mock-keychain`, so Chromium never asks the
login keychain (a prompt would block an unattended run). It then requires,
each within a bound:

1. an `opened` reply for the boot request's key and profile, whose cache path
   is `<case>/cef-profiles/<profile>` and whose native pid is the started one;
2. with `--expect-load` (a `fixture-inspection` browser only), the browser's
   main-frame load marker, which shows a renderer process ran;
3. with `--reuse`, an `opened` reply for a second Open of the same key that
   reuses the same native browser and window, and that the raise succeeded:
   no refusal in the browser's stderr and, on Windows, the reported top-level
   window is the foreground window;
4. after `{"operation":"shutdown","id":N}`, a `closed` reply for that browser,
   a `stopped` reply for N and process exit code 0;
5. no process left: none still in the Job Object (Windows) or the process
   group (Linux, macOS), and none of the recorded descendants alive (macOS),
   within 10 s of the exit;
6. with `--expect-sandbox` (Windows, the sandboxed browser of W7), before the
   shutdown: at least one renderer process in the job, each without
   `--no-sandbox` and with a token integrity level of Low or Untrusted,
   below the browser's own (read from its token), the browser's own
   command line read and without `--no-sandbox` either, and no other process
   in the job whose command line could not be read unless it had already
   exited or ends within 1 s (one being terminated can lose its readable
   command line while it still reads as running); recorded as `sandbox`
   (with those unread processes). The
   observer of the actual App (desktop/windows_browser_observer.py) applies
   the same check to each profile's browser and its process tree. On macOS
   (M1) it requires at least one renderer among the browser's descendants,
   each in the seatbelt sandbox (`sandbox_check(pid, NULL, 0)` from libSystem
   answers 1) and without `--no-sandbox`, and no `--no-sandbox` for the
   browser;
7. on Windows, the browser's own shutdown line from its log (W7: child
   processes still running after CefShutdown, how it ends, its exit code),
   recorded as `shutdown`.

Requests use browser_process::Request's serde form (compact JSON, one line).
It prints one JSON summary on stdout, writes it to `<case>/summary.json` and
exits 1 on any failure, printing the tails of the browser's logs. Leftover
processes are ended (closing the Job Object, or SIGKILL to the group). The
case directory is never removed.
"""
import argparse
import ctypes
import datetime
import hashlib
import json
import os
import queue
import re
import signal
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from pathlib import Path

MAX_PACKET_BYTES = 64 * 1024  # browser_process::MAX_PACKET_BYTES
WINDOWS = os.name == "nt"
LINUX = sys.platform.startswith("linux")
MACOS = sys.platform == "darwin"
LIBRARY = "libcef.dll" if WINDOWS else "libcef.so"
# macOS (M1): what the app bundle's Contents/Frameworks must hold (its
# `--runtime`): CEF's framework and the helper apps CEF starts its processes
# from (scripts/bundle-macos-cef.mjs makes all five).
MACOS_RUNTIME = ("Chromium Embedded Framework.framework/Chromium Embedded Framework",
                 "Chromium Embedded Framework.framework/Libraries/libcef_sandbox.dylib",
                 "takosumi-cef-browser-process Helper.app/Contents/MacOS/takosumi-cef-browser-process Helper",
                 "takosumi-cef-browser-process Helper (Renderer).app/Contents/MacOS/"
                 "takosumi-cef-browser-process Helper (Renderer)",
                 "takosumi-cef-browser-process Helper (GPU).app/Contents/MacOS/takosumi-cef-browser-process Helper (GPU)")
# A Unix socket path's limit: 108 bytes on Linux, 104 on macOS.
SOCKET_LIMIT = 104 if MACOS else 108
# A sockaddr_un path holds 107 bytes; Chromium puts its singleton socket in TMPDIR.
SOCKET_SUFFIX = "/org.chromium.Chromium.XXXXXX/SingletonSocket"
# The fixed switches process_adapter.rs passes (--password-store is Linux only).
APP_SWITCHES = ["--no-first-run", "--no-default-browser-check", "--disable-background-networking",
                "--disable-component-update", "--disable-sync"]
OPENED_FIELDS = {"event", "id", "key", "profile", "browser_id", "cache_path", "window_id", "native_pid"}
URL_SHAPE = re.compile(r"https://(?:[a-z0-9-]+\.)*[a-z0-9-]+(?::[0-9]{1,5})?/")
RAISE_REFUSED = "Reused browser window was not raised:"
LEFTOVER_WAIT = 10


class Failure(Exception):
    pass


def require(condition, message):
    if not condition:
        raise Failure(message)


# Win32 structures with explicit field widths (DWORD = 32 bits, SIZE_T and
# ULONG_PTR = pointer width), so their x64 sizes can be checked anywhere.
class BasicLimit(ctypes.Structure):  # JOBOBJECT_BASIC_LIMIT_INFORMATION
    _fields_ = [("PerProcessUserTimeLimit", ctypes.c_int64), ("PerJobUserTimeLimit", ctypes.c_int64),
                ("LimitFlags", ctypes.c_uint32), ("MinimumWorkingSetSize", ctypes.c_size_t),
                ("MaximumWorkingSetSize", ctypes.c_size_t), ("ActiveProcessLimit", ctypes.c_uint32),
                ("Affinity", ctypes.c_size_t), ("PriorityClass", ctypes.c_uint32),
                ("SchedulingClass", ctypes.c_uint32)]


class IoCounters(ctypes.Structure):  # IO_COUNTERS
    _fields_ = [(name, ctypes.c_uint64) for name in (
        "ReadOperationCount", "WriteOperationCount", "OtherOperationCount",
        "ReadTransferCount", "WriteTransferCount", "OtherTransferCount")]


class ExtendedLimit(ctypes.Structure):  # JOBOBJECT_EXTENDED_LIMIT_INFORMATION
    _fields_ = [("BasicLimitInformation", BasicLimit), ("IoInfo", IoCounters),
                ("ProcessMemoryLimit", ctypes.c_size_t), ("JobMemoryLimit", ctypes.c_size_t),
                ("PeakProcessMemoryUsed", ctypes.c_size_t), ("PeakJobMemoryUsed", ctypes.c_size_t)]


class Accounting(ctypes.Structure):  # JOBOBJECT_BASIC_ACCOUNTING_INFORMATION
    _fields_ = [("TotalUserTime", ctypes.c_int64), ("TotalKernelTime", ctypes.c_int64),
                ("ThisPeriodTotalUserTime", ctypes.c_int64), ("ThisPeriodTotalKernelTime", ctypes.c_int64),
                ("TotalPageFaultCount", ctypes.c_uint32), ("TotalProcesses", ctypes.c_uint32),
                ("ActiveProcesses", ctypes.c_uint32), ("TotalTerminatedProcesses", ctypes.c_uint32)]


class ThreadEntry(ctypes.Structure):  # THREADENTRY32
    _fields_ = [("dwSize", ctypes.c_uint32), ("cntUsage", ctypes.c_uint32), ("th32ThreadID", ctypes.c_uint32),
                ("th32OwnerProcessID", ctypes.c_uint32), ("tpBasePri", ctypes.c_int32),
                ("tpDeltaPri", ctypes.c_int32), ("dwFlags", ctypes.c_uint32)]


class ProcessEntry(ctypes.Structure):  # PROCESSENTRY32W
    _fields_ = [("dwSize", ctypes.c_uint32), ("cntUsage", ctypes.c_uint32), ("th32ProcessID", ctypes.c_uint32),
                ("th32DefaultHeapID", ctypes.c_size_t), ("th32ModuleID", ctypes.c_uint32),
                ("cntThreads", ctypes.c_uint32), ("th32ParentProcessID", ctypes.c_uint32),
                ("pcPriClassBase", ctypes.c_int32), ("dwFlags", ctypes.c_uint32),
                ("szExeFile", ctypes.c_wchar * 260)]


X64_SIZES = {BasicLimit: 64, IoCounters: 48, ExtendedLimit: 144, Accounting: 48, ThreadEntry: 28,
             ProcessEntry: 568}

CREATE_SUSPENDED = 0x4
JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x2000
JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION = 1
JOB_OBJECT_EXTENDED_LIMIT_INFORMATION = 9
PROCESS_TERMINATE = 0x1
PROCESS_SET_QUOTA = 0x100
THREAD_SUSPEND_RESUME = 0x2
TH32CS_SNAPTHREAD = 0x4
INVALID_HANDLE_VALUE = ctypes.c_void_p(-1).value
GA_ROOT = 2


def win32():
    """kernel32 and user32 with the prototypes used here (Windows only)."""
    handle, boolean, dword = ctypes.c_void_p, ctypes.c_int, ctypes.c_uint32
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    for library, name, restype, argtypes in (
        (kernel32, "CreateJobObjectW", handle, [ctypes.c_void_p, ctypes.c_wchar_p]),
        (kernel32, "SetInformationJobObject", boolean, [handle, ctypes.c_int, ctypes.c_void_p, dword]),
        (kernel32, "QueryInformationJobObject", boolean,
         [handle, ctypes.c_int, ctypes.c_void_p, dword, ctypes.c_void_p]),
        (kernel32, "AssignProcessToJobObject", boolean, [handle, handle]),
        (kernel32, "TerminateJobObject", boolean, [handle, ctypes.c_uint]),
        (kernel32, "OpenProcess", handle, [dword, boolean, dword]),
        (kernel32, "OpenThread", handle, [dword, boolean, dword]),
        (kernel32, "ResumeThread", dword, [handle]),
        (kernel32, "CreateToolhelp32Snapshot", handle, [dword, dword]),
        (kernel32, "Thread32First", boolean, [handle, ctypes.POINTER(ThreadEntry)]),
        (kernel32, "Thread32Next", boolean, [handle, ctypes.POINTER(ThreadEntry)]),
        (kernel32, "CloseHandle", boolean, [handle]),
        (user32, "GetForegroundWindow", handle, []),
        (user32, "GetAncestor", handle, [handle, ctypes.c_uint]),
    ):
        function = getattr(library, name)
        function.restype, function.argtypes = restype, argtypes
    return kernel32, user32


def last_error(what):
    code = ctypes.get_last_error()
    return OSError(f"{what} failed (Windows error {code})")


# Token integrity levels (the mandatory label's RID).
INTEGRITY = {0x0000: "untrusted", 0x1000: "low", 0x2000: "medium", 0x2100: "medium-plus", 0x3000: "high",
             0x4000: "system"}
LOW_INTEGRITY = 0x1000
# OpenProcess's error for a process id that no longer names a process.
ERROR_INVALID_PARAMETER = 87
STILL_ACTIVE = 259
TH32CS_SNAPPROCESS = 0x2


def windows_arguments(line):
    """A Windows command line split as the C runtime does (the program name
    first, without backslash escapes; then quotes, 2n/2n+1 backslashes before
    a quote, and "" inside quotes), as process_type.rs splits it."""
    arguments, index, program, quoted = [], 0, [], False
    while index < len(line) and (quoted or line[index] not in " \t"):
        if line[index] == '"':
            quoted = not quoted
        else:
            program.append(line[index])
        index += 1
    arguments.append("".join(program))
    while True:
        while index < len(line) and line[index] in " \t":
            index += 1
        if index >= len(line):
            return arguments
        argument, quoted = [], False
        while index < len(line):
            c = line[index]
            if c == "\\":
                start = index
                while index < len(line) and line[index] == "\\":
                    index += 1
                count = index - start
                if index < len(line) and line[index] == '"':
                    argument.append("\\" * (count // 2))
                    if count % 2:
                        argument.append('"')
                        index += 1
                else:
                    argument.append("\\" * count)
                continue
            if c == '"':
                if quoted and index + 1 < len(line) and line[index + 1] == '"':
                    argument.append('"')
                    index += 2
                    continue
                quoted = not quoted
                index += 1
                continue
            if not quoted and c in " \t":
                break
            argument.append(c)
            index += 1
        arguments.append("".join(argument))


def process_type(arguments):
    """The Chromium process type of split arguments (None: the browser
    process); switches end at a bare `--`."""
    kind = None
    for argument in arguments[1:]:
        if argument == "--":
            break
        if argument.startswith("--type="):
            kind = argument[len("--type="):]
    return kind


def has_no_sandbox(arguments):
    rest = arguments[1:]
    return "--no-sandbox" in (rest[:rest.index("--")] if "--" in rest else rest)


# How long a process whose command line could not be read is given to end:
# one being terminated can lose its readable command line while its exit
# code still reads STILL_ACTIVE.
UNREAD_WAIT = 1.0


def settle_unread(unread, browser_pid, ended):
    """`unread` with each live process other than the browser given
    `ended(pid)` (it waits up to UNREAD_WAIT for the process to end): one
    that ends then counts as exited (`endedAfterWait`)."""
    settled = []
    for process in unread:
        if not process.get("exited") and process.get("pid") != browser_pid and ended(process["pid"]):
            process = dict(process, exited=True, endedAfterWait=True)
        settled.append(process)
    return settled


def ended_within(kernel32, pid, seconds):
    """Whether a process ends within `seconds` (Windows): reopened with
    SYNCHRONIZE and waited for; an id that no longer opens
    (ERROR_INVALID_PARAMETER) has ended."""
    process = kernel32.OpenProcess(0x00100000, False, pid)  # SYNCHRONIZE
    if not process:
        return ctypes.get_last_error() == ERROR_INVALID_PARAMETER
    try:
        return kernel32.WaitForSingleObject(process, int(seconds * 1000)) == 0  # WAIT_OBJECT_0
    finally:
        kernel32.CloseHandle(process)


def sandbox_problems(browser, renderers, unread=()):
    """Why the sandbox is not shown (empty when it is): `browser` and each
    of `renderers` are {"integrity": RID or None, "noSandbox": bool}; the
    browser's `argumentsRead` is False when its command line was not read.
    Each of `unread` ({"pid", "error", "exited"}) is a process whose command
    line was not read, so whose type is unknown: a problem unless it had
    already exited, or ended within UNREAD_WAIT (settle_unread; the
    browser's own is reported above)."""
    problems = []
    for process in unread:
        if not process.get("exited") and process.get("pid") != browser.get("pid"):
            problems.append(f"process {process.get('pid')}: command line not read ({process.get('error')})")
    if browser.get("argumentsRead") is False:
        problems.append(f"the browser's command line was not read ({browser.get('error')})")
    if browser.get("noSandbox"):
        problems.append("the browser runs with --no-sandbox")
    if browser.get("integrity") is None:
        problems.append("the browser's integrity level is unknown")
    if not renderers:
        problems.append("no renderer process was found")
    for renderer in renderers:
        integrity = renderer.get("integrity")
        if renderer.get("noSandbox"):
            problems.append(f"renderer {renderer.get('pid')} runs with --no-sandbox")
        if integrity is None:
            problems.append(f"renderer {renderer.get('pid')}: integrity level unknown ({renderer.get('error')})")
        elif integrity > LOW_INTEGRITY or (browser.get("integrity") is not None
                                           and integrity >= browser["integrity"]):
            problems.append(f"renderer {renderer.get('pid')} runs at {INTEGRITY.get(integrity, hex(integrity))} "
                            f"integrity, not below the browser's and at most low")
    return problems


def process_facts(kernel32, advapi32, ntdll, pid):
    """A process's split command line and token integrity RID (Windows):
    {"pid", "arguments", "integrity", "error"?, "exited"?}. When either was
    not read, `exited` says whether the process had already exited: its id
    no longer opens (ERROR_INVALID_PARAMETER), or it has an exit code."""
    facts = {"pid": pid, "arguments": None, "integrity": None}
    process = kernel32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not process:
        error = ctypes.get_last_error()
        facts["error"] = f"OpenProcess: Windows error {error}"
        facts["exited"] = error == ERROR_INVALID_PARAMETER
        return facts
    try:
        size = ctypes.c_uint32(0)
        buffer = (ctypes.c_uint64 * 8192)()  # 64 KiB, aligned for UNICODE_STRING
        status = ntdll.NtQueryInformationProcess(process, 60, buffer, ctypes.sizeof(buffer), ctypes.byref(size))
        if status == 0:
            length = ctypes.c_uint16.from_buffer(buffer, 0).value
            address = ctypes.c_void_p.from_buffer(buffer, 8).value
            facts["arguments"] = windows_arguments(ctypes.wstring_at(address, length // 2)) if address else []
        else:
            facts["error"] = f"NtQueryInformationProcess: 0x{status & 0xFFFFFFFF:08X}"
        token = ctypes.c_void_p()
        if advapi32.OpenProcessToken(process, 0x8, ctypes.byref(token)):  # TOKEN_QUERY
            try:
                label = (ctypes.c_uint64 * 16)()
                needed = ctypes.c_uint32(0)
                # TokenIntegrityLevel: a TOKEN_MANDATORY_LABEL, its SID first.
                if advapi32.GetTokenInformation(token, 25, label, ctypes.sizeof(label), ctypes.byref(needed)):
                    sid = ctypes.c_void_p.from_buffer(label, 0)
                    count = advapi32.GetSidSubAuthorityCount(sid)[0]
                    facts["integrity"] = advapi32.GetSidSubAuthority(sid, count - 1)[0]
                else:
                    facts["error"] = f"GetTokenInformation: Windows error {ctypes.get_last_error()}"
            finally:
                kernel32.CloseHandle(token)
        else:
            facts["error"] = f"OpenProcessToken: Windows error {ctypes.get_last_error()}"
        if facts["arguments"] is None or facts["integrity"] is None:
            code = ctypes.c_uint32(0)
            facts["exited"] = bool(kernel32.GetExitCodeProcess(process, ctypes.byref(code))
                                   and code.value != STILL_ACTIVE)
    finally:
        kernel32.CloseHandle(process)
    return facts


# The browser's own line on its standard error after CefShutdown (Windows,
# W7; src/children_windows.rs): its pid, how many child processes were still
# running (and how many it could not open), how it ends ("exit-process" for
# the sandboxed browser, "return" for the unsandboxed candidate) and its exit
# code.
BROWSER_SHUTDOWN = "takosumi-cef-browser-process: shutdown "


def parse_ps(text):
    """`ps -ww -A -o pid=,ppid=,command=` output as (pid, ppid, command)."""
    rows = []
    for line in text.splitlines():
        parts = line.split(None, 2)
        if len(parts) >= 2 and parts[0].isdigit() and parts[1].isdigit():
            rows.append((int(parts[0]), int(parts[1]), parts[2] if len(parts) > 2 else ""))
    return rows


def ps_rows():
    """macOS: every process's (pid, ppid, command line) from `ps`."""
    result = subprocess.run(["ps", "-ww", "-A", "-o", "pid=,ppid=,command="], capture_output=True, text=True,
                            timeout=30, check=True)
    return parse_ps(result.stdout)


def posix_tree(root, rows=None):
    """`root` and its descendants by parent pid, each as (pid, command
    line); the command line split on spaces is enough for its switches
    (`--type=`, `--no-sandbox`), not for a program path with spaces."""
    rows = ps_rows() if rows is None else rows
    commands = {pid: command for pid, _, command in rows}
    return [(pid, commands.get(pid, "")) for pid in [root] + descendant_pids([(pid, ppid) for pid, ppid, _ in rows], root)]


def seatbelt_sandboxed(pid):
    """macOS: whether `pid` runs in a seatbelt sandbox (libSystem's
    `sandbox_check(pid, NULL, 0)`: 1 sandboxed, 0 not); None when it cannot
    tell (the call failed, or the process is gone)."""
    try:
        check = ctypes.CDLL("/usr/lib/libSystem.B.dylib").sandbox_check
    except (OSError, AttributeError):
        return None
    check.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int]
    check.restype = ctypes.c_int
    answer = check(pid, None, 0)
    return {1: True, 0: False}.get(answer)


def seatbelt_problems(browser, renderers):
    """Why the macOS sandbox is not shown (empty when it is): the browser
    and each renderer are {"pid", "noSandbox", "sandboxed"?}."""
    problems = []
    if browser.get("noSandbox"):
        problems.append("the browser runs with --no-sandbox")
    if not renderers:
        problems.append("no renderer process was found")
    for renderer in renderers:
        if renderer.get("noSandbox"):
            problems.append(f"renderer {renderer.get('pid')} runs with --no-sandbox")
        if renderer.get("sandboxed") is not True:
            problems.append(f"renderer {renderer.get('pid')} is not in the seatbelt sandbox "
                            f"(sandbox_check: {renderer.get('sandboxed')})")
    return problems


def seatbelt_evidence(browser_pid, deadline, tree=posix_tree, sandboxed=seatbelt_sandboxed):
    """macOS: the browser's descendants once at least one renderer is among
    them (until `deadline`), each renderer's sandbox state; `problems` says
    why they do not show the sandbox (empty when they do)."""
    while True:
        processes = [(pid, command.split()) for pid, command in tree(browser_pid)]
        renderers = [(pid, words) for pid, words in processes
                     if pid != browser_pid and process_type(words) == "renderer"]
        if renderers or time.monotonic() >= deadline:
            break
        time.sleep(0.5)
    browser_words = next((words for pid, words in processes if pid == browser_pid), [])
    evidence = {"browser": {"pid": browser_pid, "noSandbox": has_no_sandbox(browser_words) if browser_words else False,
                            "sandboxed": sandboxed(browser_pid)},
                "renderers": [{"pid": pid, "noSandbox": has_no_sandbox(words), "sandboxed": sandboxed(pid)}
                              for pid, words in renderers],
                "types": sorted({process_type(words) or "browser" for _, words in processes if words})}
    evidence["problems"] = seatbelt_problems(evidence["browser"], evidence["renderers"])
    return evidence


def alive(pid):
    """Whether a process id still names a process (also a zombie)."""
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def browser_shutdown(text, pid=None):
    """The browser's shutdown lines in a log, parsed (with `pid`, only that
    browser's); one that does not parse is kept as `unparsed`."""
    found = []
    for line in text.splitlines():
        index = line.find(BROWSER_SHUTDOWN)
        if index < 0:
            continue
        payload = line[index + len(BROWSER_SHUTDOWN):]
        try:
            parsed = json.loads(payload)
        except ValueError:
            found.append({"unparsed": payload[:500]})
            continue
        if pid is None or (isinstance(parsed, dict) and parsed.get("pid") == pid):
            found.append(parsed)
    return found


def descendant_pids(pairs, root, created=lambda pid: None):
    """The ids of `root`'s descendants, from (pid, parent pid) pairs. With
    `created(pid)` (a creation time, or None when unknown), a process counts
    as its parent's child only if it was not created before that parent: an
    older process whose parent id merely names a reused id is not one (as in
    src/children_windows.rs)."""
    children = {}
    for pid, parent in pairs:
        if pid != parent:
            children.setdefault(parent, []).append(pid)
    found, queue, seen = [], [root], {root}
    while queue:
        parent = queue.pop()
        for child in children.get(parent, ()):
            if child in seen:
                continue
            born, parent_born = created(child), created(parent)
            if born is not None and parent_born is not None and born < parent_born:
                continue
            seen.add(child)
            found.append(child)
            queue.append(child)
    return sorted(found)


class ProcessTree:
    """A browser process and its descendants by parent process id (a
    Toolhelp snapshot), the processes sandbox_evidence reads where no job of
    the caller's own holds just that browser: in the App, each browser runs
    in its guard's job inside the App's, and Chromium puts its sandboxed
    processes in jobs of their own below those (Windows)."""

    def __init__(self, kernel32, root):
        require(not WINDOWS or ctypes.sizeof(ctypes.c_void_p) != 8
                or ctypes.sizeof(ProcessEntry) == X64_SIZES[ProcessEntry], "Unexpected ProcessEntry layout")
        self.kernel32, self.root = kernel32, root
        handle = ctypes.c_void_p
        kernel32.CreateToolhelp32Snapshot.restype = handle
        kernel32.CreateToolhelp32Snapshot.argtypes = [ctypes.c_uint32, ctypes.c_uint32]
        for name in ("Process32FirstW", "Process32NextW"):
            function = getattr(kernel32, name)
            function.restype, function.argtypes = ctypes.c_int, [handle, ctypes.POINTER(ProcessEntry)]
        time = ctypes.POINTER(ctypes.c_uint64)
        kernel32.GetProcessTimes.restype = ctypes.c_int
        kernel32.GetProcessTimes.argtypes = [handle, time, time, time, time]

    def created(self, pid):
        """A process's creation time (FILETIME as one number), or None."""
        process = self.kernel32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
        if not process:
            return None
        try:
            times = [ctypes.c_uint64(0) for _ in range(4)]
            if not self.kernel32.GetProcessTimes(process, *(ctypes.byref(value) for value in times)):
                return None
            return times[0].value
        finally:
            self.kernel32.CloseHandle(process)

    def entries(self):
        """Every process's (pid, parent pid, image file name), from a
        Toolhelp snapshot."""
        snapshot = self.kernel32.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
        if snapshot in (None, INVALID_HANDLE_VALUE):
            raise last_error("CreateToolhelp32Snapshot")
        try:
            found = []
            entry = ProcessEntry(dwSize=ctypes.sizeof(ProcessEntry))
            more = self.kernel32.Process32FirstW(snapshot, ctypes.byref(entry))
            while more:
                found.append((entry.th32ProcessID, entry.th32ParentProcessID, entry.szExeFile))
                entry.dwSize = ctypes.sizeof(ProcessEntry)
                more = self.kernel32.Process32NextW(snapshot, ctypes.byref(entry))
            return found
        finally:
            self.kernel32.CloseHandle(snapshot)

    def pairs(self):
        return [(pid, parent) for pid, parent, _ in self.entries()]

    def process_ids(self):
        times = {}

        def created(pid):
            if pid not in times:
                times[pid] = self.created(pid)
            return times[pid]

        return [self.root] + descendant_pids(self.pairs(), self.root, created)


# FILETIME ticks (100 ns since 1601-01-01 UTC) at the Unix epoch.
FILETIME_UNIX_EPOCH = 116444736000000000


def filetime_iso(value):
    """A FILETIME (100 ns ticks since 1601-01-01, UTC) as ISO 8601 UTC with
    milliseconds, or None for none (0 or None)."""
    if not value:
        return None
    moment = (datetime.datetime(1970, 1, 1, tzinfo=datetime.timezone.utc)
              + datetime.timedelta(microseconds=(value - FILETIME_UNIX_EPOCH) // 10))
    return moment.strftime("%Y-%m-%dT%H:%M:%S.") + f"{moment.microsecond // 1000:03d}Z"


def describe_processes(records):
    """Processes for a failure message, one after another: pid, image file
    name, parent pid and creation time (`?` for what is unknown); no
    command line."""
    unknown = lambda value: "?" if value is None else value
    return "; ".join(f"{record.get('pid')} {unknown(record.get('image'))} (parent {unknown(record.get('parent'))}, "
                     f"created {unknown(record.get('created'))})" for record in records)


def process_records(kernel32, pids):
    """Windows: each of `pids` with its image file name and parent pid (a
    Toolhelp process snapshot) and its creation time (GetProcessTimes, ISO
    8601 UTC); None for what cannot be read (a process gone meanwhile). No
    command line is read."""
    tree = ProcessTree(kernel32, 0)
    try:
        listed = {pid: (parent, image) for pid, parent, image in tree.entries()}
    except OSError:
        listed = {}
    records = []
    for pid in pids:
        parent, image = listed.get(pid, (None, None))
        records.append({"pid": pid, "image": image, "parent": parent, "created": filetime_iso(tree.created(pid))})
    return records


def sandbox_evidence(job, browser_pid, deadline):
    """Windows: the browser's and its renderers' integrity levels and
    command lines, once at least one renderer is among `job.process_ids()`
    (until `deadline`): `job` is a WindowsJob holding just the browser, or a
    ProcessTree of it, with its `kernel32`. The `problems` it returns say
    why they do not show Chromium's sandbox (empty when they do)."""
    handle, dword = ctypes.c_void_p, ctypes.c_uint32
    job.kernel32.GetExitCodeProcess.restype = ctypes.c_int
    job.kernel32.GetExitCodeProcess.argtypes = [handle, ctypes.POINTER(dword)]
    job.kernel32.WaitForSingleObject.restype = dword
    job.kernel32.WaitForSingleObject.argtypes = [handle, dword]
    advapi32 = ctypes.WinDLL("advapi32", use_last_error=True)
    ntdll = ctypes.WinDLL("ntdll")
    advapi32.OpenProcessToken.argtypes = [handle, dword, ctypes.POINTER(handle)]
    advapi32.GetTokenInformation.argtypes = [handle, ctypes.c_int, ctypes.c_void_p, dword, ctypes.POINTER(dword)]
    advapi32.GetSidSubAuthorityCount.argtypes = [handle]
    advapi32.GetSidSubAuthorityCount.restype = ctypes.POINTER(ctypes.c_ubyte)
    advapi32.GetSidSubAuthority.argtypes = [handle, dword]
    advapi32.GetSidSubAuthority.restype = ctypes.POINTER(dword)
    ntdll.NtQueryInformationProcess.argtypes = [handle, ctypes.c_int, ctypes.c_void_p, dword, ctypes.POINTER(dword)]
    ntdll.NtQueryInformationProcess.restype = ctypes.c_int32
    while True:
        facts = [process_facts(job.kernel32, advapi32, ntdll, pid) for pid in job.process_ids()]
        renderers = [fact for fact in facts if fact["arguments"] and process_type(fact["arguments"]) == "renderer"]
        if renderers or time.monotonic() >= deadline:
            break
        time.sleep(0.5)
    browser = next((fact for fact in facts if fact["pid"] == browser_pid), {"pid": browser_pid})
    summarize = lambda fact: {"pid": fact["pid"], "integrity": fact.get("integrity"),
                              "integrityName": INTEGRITY.get(fact.get("integrity")),
                              "argumentsRead": fact.get("arguments") is not None,
                              "noSandbox": bool(fact.get("arguments")) and has_no_sandbox(fact["arguments"]),
                              **({"error": fact["error"]} if "error" in fact else {})}
    evidence = {"browser": summarize(browser), "renderers": [summarize(fact) for fact in renderers],
                "types": sorted({process_type(fact["arguments"]) or "browser" for fact in facts
                                 if fact["arguments"]}),
                # Processes whose command line could not be read (so their
                # type is unknown), with the reason.
                "unread": settle_unread(
                    [{"pid": fact["pid"], "error": fact.get("error"), "exited": fact.get("exited", False)}
                     for fact in facts if fact.get("arguments") is None],
                    browser_pid, lambda pid: ended_within(job.kernel32, pid, UNREAD_WAIT))}
    evidence["problems"] = sandbox_problems(evidence["browser"], evidence["renderers"], evidence["unread"])
    return evidence


class WindowsJob:
    """A kill-on-close Job Object that owns the browser and every process it starts."""

    def __init__(self):
        for structure, size in X64_SIZES.items():
            require(ctypes.sizeof(structure) == size, f"Unexpected {structure.__name__} layout")
        self.kernel32, self.user32 = win32()
        self.handle = self.kernel32.CreateJobObjectW(None, None)
        if not self.handle:
            raise last_error("CreateJobObjectW")
        limits = ExtendedLimit()
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        if not self.kernel32.SetInformationJobObject(self.handle, JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                                                     ctypes.byref(limits), ctypes.sizeof(limits)):
            error = last_error("SetInformationJobObject")
            self.close()
            raise error

    def adopt(self, pid):
        """Assign a process started suspended, then resume its thread."""
        process = self.kernel32.OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, False, pid)
        if not process:
            raise last_error("OpenProcess")
        try:
            if not self.kernel32.AssignProcessToJobObject(self.handle, process):
                raise last_error("AssignProcessToJobObject")
        finally:
            self.kernel32.CloseHandle(process)
        snapshot = self.kernel32.CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)
        if snapshot in (None, INVALID_HANDLE_VALUE):
            raise last_error("CreateToolhelp32Snapshot")
        resumed = 0
        try:
            entry = ThreadEntry(dwSize=ctypes.sizeof(ThreadEntry))
            found = self.kernel32.Thread32First(snapshot, ctypes.byref(entry))
            while found:
                if entry.th32OwnerProcessID == pid:
                    thread = self.kernel32.OpenThread(THREAD_SUSPEND_RESUME, False, entry.th32ThreadID)
                    if not thread:
                        raise last_error("OpenThread")
                    try:
                        if self.kernel32.ResumeThread(thread) == 0xFFFFFFFF:
                            raise last_error("ResumeThread")
                    finally:
                        self.kernel32.CloseHandle(thread)
                    resumed += 1
                entry.dwSize = ctypes.sizeof(ThreadEntry)
                found = self.kernel32.Thread32Next(snapshot, ctypes.byref(entry))
        finally:
            self.kernel32.CloseHandle(snapshot)
        require(resumed >= 1, "The suspended browser had no thread to resume")

    def processes(self):
        """(processes ever in the job, processes still in it)."""
        info = Accounting()
        if not self.kernel32.QueryInformationJobObject(self.handle, JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION,
                                                       ctypes.byref(info), ctypes.sizeof(info), None):
            raise last_error("QueryInformationJobObject")
        return info.TotalProcesses, info.ActiveProcesses

    def process_ids(self):
        """The ids of the processes now in the job (the first 1024)."""
        class IdList(ctypes.Structure):  # JOBOBJECT_BASIC_PROCESS_ID_LIST
            _fields_ = [("assigned", ctypes.c_uint32), ("listed", ctypes.c_uint32),
                        ("ids", ctypes.c_size_t * 1024)]

        ids = IdList()
        if not self.kernel32.QueryInformationJobObject(self.handle, 3, ctypes.byref(ids), ctypes.sizeof(ids), None):
            raise last_error("QueryInformationJobObject")
        return [ids.ids[index] for index in range(min(ids.listed, 1024))]

    def terminate(self):
        if self.handle and not self.kernel32.TerminateJobObject(self.handle, 1):
            raise last_error("TerminateJobObject")

    def close(self):
        """Closing the last handle kills whatever is still in the job."""
        if self.handle:
            self.kernel32.CloseHandle(self.handle)
            self.handle = None


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as file:
        for chunk in iter(lambda: file.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_new(path, data):
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_BINARY", 0) | getattr(os, "O_NOFOLLOW", 0)
    with os.fdopen(os.open(path, flags, 0o600), "wb") as file:
        file.write(data)


def packet(value):
    """One request line in browser_process::Request's serde_json form."""
    data = json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    require(len(data) <= MAX_PACKET_BYTES and b"\n" not in data, "Request exceeds the pipe packet bound")
    return data


def open_request(request_id, profile, url, resume):
    # Field order of Request::Open and OpenRequest; omitted fields are the
    # ones serde skips when empty. The key is the Web home's screen identity.
    origin = url[:-1]
    return {"operation": "open", "id": request_id, "payload": {
        "key": f"web-home:{profile}:{origin}", "url": url, "title": "Windows smoke", "profile": profile,
        "webHome": True, "resume": resume}}


def tail(path, limit=16384):
    try:
        with open(path, "rb") as file:
            file.seek(0, os.SEEK_END)
            size = file.tell()
            file.seek(max(0, size - limit))
            return file.read().decode("utf-8", "replace")
    except OSError as error:
        return f"<unreadable: {error}>"


class Browser:
    def __init__(self, command, case, env):
        self.case = case
        self.log = open(case / "browser.log", "xb")
        self.protocol = open(case / "protocol.jsonl", "xb")
        self.job = None
        if WINDOWS:
            self.job = WindowsJob()
            extra = {"creationflags": CREATE_SUSPENDED}
        else:
            extra = {"start_new_session": True}
        try:
            self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                            stderr=self.log, cwd=case, env=env, **extra)
        except BaseException:
            if self.job is not None:
                self.job.close()
            raise
        if self.job is not None:
            try:
                self.job.adopt(self.process.pid)
            except BaseException:
                self.process.kill()
                self.process.wait()
                self.job.close()
                raise
        # macOS: the browser's descendants recorded before the shutdown, each
        # of which must have ended afterwards (an orphan leaves the tree).
        self.descendants = []
        self.lines = queue.Queue()
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()
        self.events = 0
        self.closed = []

    def _read(self):
        try:
            while True:
                line = self.process.stdout.readline(MAX_PACKET_BYTES + 2)
                if not line:
                    break
                self.lines.put(line)
        finally:
            self.lines.put(None)

    def send(self, value):
        data = packet(value)
        self.process.stdin.write(data + b"\n")
        self.process.stdin.flush()

    def pump(self, deadline, what):
        """The next protocol event, or None when nothing arrived in 0.25 s."""
        remaining = deadline - time.monotonic()
        require(remaining > 0, f"No {what} within the bound")
        try:
            line = self.lines.get(timeout=min(remaining, 0.25))
        except queue.Empty:
            return None
        if line is None:
            self.lines.put(None)
            raise Failure(f"Browser stdout closed before {what} (exit {self.process.poll()})")
        self.protocol.write(line if line.endswith(b"\n") else line + b"\n")
        self.protocol.flush()
        require(line.endswith(b"\n") and len(line) - 1 <= MAX_PACKET_BYTES, "Browser wrote an unbounded line")
        try:
            event = json.loads(line[:-1].decode("utf-8"))
        except ValueError:
            raise Failure(f"Browser wrote a non-protocol line: {line[:200]!r}") from None
        require(isinstance(event, dict) and isinstance(event.get("event"), str), "Browser reply has no event")
        self.events += 1
        return event

    def unsolicited(self, event):
        kind = event["event"]
        if kind == "openExternal":
            return
        if kind == "closed":
            self.closed.append(event)
            return
        raise Failure(f"Unexpected browser reply: {json.dumps(event)[:500]}")

    def expect(self, kind, request_id, deadline):
        while True:
            event = self.pump(deadline, f"{kind} reply for request {request_id}")
            if event is None:
                continue
            if event["event"] == kind and event.get("id") == request_id:
                return event
            self.unsolicited(event)

    def wait_until(self, condition, what, deadline):
        while True:
            found = condition()
            if found is not None:
                return found
            event = self.pump(deadline, what)
            if event is not None:
                self.unsolicited(event)

    def record_descendants(self):
        """macOS: remember the browser's descendants now (by parent pid)."""
        self.descendants = [pid for pid, _ in posix_tree(self.process.pid) if pid != self.process.pid]
        return self.descendants

    def leftovers(self):
        """Processes still owned after the browser exited, waiting up to
        LEFTOVER_WAIT for them to end: ({"total", "active"} on Windows, the
        Job Object's counts; {"groupAlive"} on Linux; on macOS also
        {"descendantsAlive"}, the recorded descendants still there)."""
        deadline = time.monotonic() + LEFTOVER_WAIT
        while True:
            if self.job is not None:
                total, active = self.job.processes()
                result = {"total": total, "active": active}
                done = active == 0
            else:
                try:
                    os.killpg(self.process.pid, 0)
                    done = False
                except ProcessLookupError:
                    done = True
                result = {"groupAlive": not done}
                if MACOS:
                    remaining = [pid for pid in self.descendants if alive(pid)]
                    result["descendantsAlive"] = remaining
                    done = done and not remaining
            if done or time.monotonic() >= deadline:
                return result, done
            time.sleep(0.5)

    def end_all(self):
        """Ends the browser and every process it started; never raises."""
        try:
            if self.job is not None:
                self.job.terminate()
            else:
                try:
                    os.killpg(self.process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                for pid in self.descendants:
                    try:
                        os.kill(pid, signal.SIGKILL)
                    except (ProcessLookupError, PermissionError):
                        pass
            self.process.wait(timeout=30)
        except (OSError, subprocess.SubprocessError) as error:
            print(f"windows_smoke: cleanup failed: {error}", file=sys.stderr)


def check_opened(event, request_id, key, profile, case, pid):
    require(set(event) == OPENED_FIELDS, f"Unexpected opened fields: {sorted(event)}")
    require(event["id"] == request_id and event["key"] == key and event["profile"] == profile,
            "The opened reply names another request, screen or profile")
    require(event["cache_path"] == str(case / "cef-profiles" / profile),
            f"Unexpected profile cache path {event['cache_path']!r}")
    require(event["native_pid"] == pid, "The opened reply comes from another process")
    require(type(event["browser_id"]) is int and type(event["window_id"]) is int and event["window_id"] != 0,
            "The opened reply has no native browser window")


def check_foreground(user32, window):
    """The reported window is top level and became the foreground window."""
    require(user32.GetAncestor(window, GA_ROOT) == window, "The reported window_id is not a top-level window")
    deadline = time.monotonic() + 2
    while True:
        foreground = user32.GetForegroundWindow()
        if foreground == window:
            return
        require(time.monotonic() < deadline,
                f"The reused window {window} is not the foreground window ({foreground})")
        time.sleep(0.1)


def load_marker(case, browser_id, pid):
    path = case / f"load-ready-{browser_id}.json"
    try:
        marker = json.loads(path.read_text())
    except (OSError, ValueError):
        return None  # absent, or still being written
    require(marker == {"pid": pid, "browserId": browser_id, "mainFrame": True},
            f"Unexpected load marker {marker!r}")
    return marker


def raise_refusal(case):
    refused = [line for line in tail(case / "browser.log").splitlines() if RAISE_REFUSED in line]
    return refused[-1][:300] if refused else None


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--browser", type=Path, required=True, help="takosumi-cef-browser-process executable")
    parser.add_argument("--runtime", type=Path,
                        help=f"directory holding {LIBRARY} and the CEF resources (default: the executable's; on "
                             "macOS the bundle's Contents/Frameworks)")
    parser.add_argument("--work", type=Path, help="parent of the new case directory (default: the system temp)")
    parser.add_argument("--url", default="https://example.com/", help="an https site root")
    parser.add_argument("--open-timeout", type=float, default=120)
    parser.add_argument("--stop-timeout", type=float, default=60)
    parser.add_argument("--expect-load", action="store_true",
                        help="require the main-frame load marker (fixture-inspection browsers only)")
    parser.add_argument("--reuse", action="store_true",
                        help="reopen the same screen and require the raise before shutting down")
    parser.add_argument("--expect-sandbox", action="store_true",
                        help="Windows (W7): require the renderers to run in Chromium's sandbox (token integrity "
                             "low or untrusted, below the browser's; no --no-sandbox); macOS (M1): each renderer "
                             "in the seatbelt sandbox")
    args = parser.parse_args()
    if not (WINDOWS or LINUX or MACOS):
        parser.error("Only Windows, Linux and macOS browsers are supported")
    if args.expect_sandbox and not (WINDOWS or MACOS):
        parser.error("--expect-sandbox reads Windows tokens or macOS sandbox state")
    if not URL_SHAPE.fullmatch(args.url) or args.url.endswith(":443/"):
        parser.error("--url must be a lowercase https site root such as https://example.com/")

    browser = Path(os.path.realpath(args.browser))
    runtime = Path(os.path.realpath(args.runtime or (browser.parent.parent / "Frameworks" if MACOS
                                                     else browser.parent)))
    summary = {"passed": False, "platform": sys.platform, "browser": str(browser), "runtime": str(runtime),
               "url": args.url, "expectLoad": args.expect_load, "reuse": args.reuse,
               "expectSandbox": args.expect_sandbox, "seconds": {}, "error": None}
    session = None
    case = None
    try:
        require(browser.is_file(), f"No browser executable at {browser}")
        if MACOS:
            require(browser.parent.name == "MacOS" and browser.parent.parent.name == "Contents",
                    f"Run the macOS browser from its app bundle (Contents/MacOS), not {browser}")
            for name in MACOS_RUNTIME:
                require((runtime / name).is_file(),
                        f"No {name} in {runtime}: assemble the bundle with scripts/bundle-macos-cef.mjs")
        else:
            for name in (LIBRARY, "icudtl.dat"):
                require((runtime / name).is_file(),
                        f"No {name} in {runtime}: build with cargo (cef-dll-sys copies the CEF runtime beside the "
                        "executable) or pass --runtime")
        summary["browserSha256"] = sha256(browser)
        work = os.path.realpath(args.work or tempfile.gettempdir())
        # realpath: the long, final spelling (no 8.3 names or links) the
        # browser requires of its directory.
        case = Path(os.path.realpath(tempfile.mkdtemp(prefix="sumi-cef-smoke-", dir=work)))
        summary["case"] = str(case)
        profile = str(uuid.uuid4())
        summary["profile"] = profile
        boot = open_request(1, profile, args.url, False)
        key = boot["payload"]["key"]
        summary["key"] = key

        env = {name: value for name, value in os.environ.items() if name != "TAKOSUMI_BROWSER_PROCESS_INSPECT"}
        env["TAKOSUMI_BROWSER_PROCESS_DIRECTORY"] = str(case)
        if args.expect_load:
            env["TAKOSUMI_BROWSER_PROCESS_INSPECT"] = "1"
        (case / "Downloads").mkdir(mode=0o700)
        (case / "tmp").mkdir(mode=0o700)
        command = [str(browser)]
        if WINDOWS:
            # libcef.dll beside the executable is found first anyway; PATH
            # covers a runtime directory given with --runtime.
            env["PATH"] = str(runtime) + os.pathsep + env.get("PATH", "")
            env["TEMP"] = env["TMP"] = str(case / "tmp")
            # No --no-sandbox: the Windows browser decides that itself.
            command += APP_SWITCHES
        elif MACOS:
            require(len(str(case / "tmp") + SOCKET_SUFFIX) < SOCKET_LIMIT, "Case path too long for Chromium's socket")
            env["TMPDIR"] = str(case / "tmp")
            # No --no-sandbox: the helpers enter the seatbelt sandbox
            # themselves. The mock keychain keeps Chromium off the login
            # keychain, whose prompt would block an unattended run.
            command += APP_SWITCHES + ["--use-mock-keychain"]
        else:
            require(len(str(case / "tmp") + SOCKET_SUFFIX) < SOCKET_LIMIT, "Case path too long for Chromium's socket")
            for variable, name in (("HOME", "home"), ("XDG_DATA_HOME", "data"), ("XDG_CONFIG_HOME", "config"),
                                   ("XDG_CACHE_HOME", "cache"), ("XDG_RUNTIME_DIR", "runtime")):
                (case / name).mkdir(mode=0o700)
                env[variable] = str(case / name)
            env["TMPDIR"] = str(case / "tmp")
            env["LD_LIBRARY_PATH"] = str(runtime)
            # As the App: Chromium refuses its sandbox to root.
            command += (["--no-sandbox"] if os.geteuid() == 0 else []) + APP_SWITCHES + ["--password-store=basic"]
        command.append("--lang=en-US")
        summary["command"] = command
        write_new(case / "boot.json", packet(boot))

        started = time.monotonic()
        session = Browser(command, case, env)
        pid = session.process.pid
        summary["pid"] = pid
        opened = session.expect("opened", 1, started + args.open_timeout)
        check_opened(opened, 1, key, profile, case, pid)
        summary["opened"] = opened
        summary["seconds"]["opened"] = round(time.monotonic() - started, 3)

        if args.expect_load:
            summary["loadReady"] = session.wait_until(
                lambda: load_marker(case, opened["browser_id"], pid), "main-frame load marker",
                time.monotonic() + args.open_timeout)
            summary["seconds"]["loadReady"] = round(time.monotonic() - started, 3)

        next_id = 2
        if args.reuse:
            session.send(open_request(next_id, profile, args.url, True))
            reused = session.expect("opened", next_id, time.monotonic() + 30)
            check_opened(reused, next_id, key, profile, case, pid)
            require(reused["browser_id"] == opened["browser_id"] and reused["window_id"] == opened["window_id"],
                    "The reopened screen did not reuse its native browser window")
            summary["reused"] = reused
            if session.job is not None:
                # The browser raised the window before it replied.
                check_foreground(session.job.user32, reused["window_id"])
                summary["foreground"] = True
            next_id += 1

        if args.expect_sandbox:
            if MACOS:
                summary["sandbox"] = seatbelt_evidence(pid, time.monotonic() + args.open_timeout)
            else:
                summary["sandbox"] = sandbox_evidence(session.job, pid, time.monotonic() + args.open_timeout)
            require(not summary["sandbox"]["problems"],
                    f"The browser's processes do not show Chromium's sandbox: {summary['sandbox']['problems']}")
        if MACOS:
            summary["descendants"] = session.record_descendants()

        stop_deadline = time.monotonic() + args.stop_timeout
        session.send({"operation": "shutdown", "id": next_id})
        summary["stopped"] = session.expect("stopped", next_id, stop_deadline)
        summary["seconds"]["stopped"] = round(time.monotonic() - started, 3)
        closed = [event for event in session.closed if event.get("browser_id") == opened["browser_id"]]
        require(len(closed) == 1 and closed[0] == {"event": "closed", "id": 0, "key": key, "profile": profile,
                                                   "browser_id": opened["browser_id"]},
                f"Expected one closed reply for the browser before stopped: {session.closed}")
        summary["closed"] = closed[0]
        try:
            code = session.process.wait(timeout=max(0.1, stop_deadline - time.monotonic()))
        except subprocess.TimeoutExpired:
            raise Failure("The browser did not exit within the bound after stopped") from None
        summary["exitCode"] = code
        summary["seconds"]["exit"] = round(time.monotonic() - started, 3)
        if WINDOWS:
            # Its own record of its end (W7): children still running after
            # CefShutdown, how it ended, and its exit code.
            summary["shutdown"] = browser_shutdown(tail(case / "browser.log"), pid)
        require(code == 0, f"The browser exited with {code}")
        session.reader.join(timeout=5)
        summary["stdoutClosed"] = not session.reader.is_alive()
        summary["leftovers"], done = session.leftovers()
        require(done, f"Processes remained {LEFTOVER_WAIT} s after the browser exited: {summary['leftovers']}")
        summary["raiseRefused"] = raise_refusal(case)
        require(not args.reuse or summary["raiseRefused"] is None,
                f"The reused window was not raised: {summary['raiseRefused']}")
        summary["passed"] = True
    except Exception as error:  # every failure ends in the summary and exit 1
        summary["error"] = str(error) if isinstance(error, Failure) else f"{type(error).__name__}: {error}"
    finally:
        if session is not None:
            if not summary["passed"]:
                if "leftovers" not in summary and session.process.poll() is not None:
                    try:
                        summary["leftovers"], _ = session.leftovers()
                    except OSError as error:
                        summary["leftovers"] = f"unknown: {error}"
                session.end_all()
                summary.setdefault("exitCode", session.process.poll())
                summary.setdefault("raiseRefused", raise_refusal(case))
            if session.job is not None:
                session.job.close()
            try:
                session.process.stdin.close()
            except OSError:
                pass
            summary["events"] = session.events
            session.log.close()
            session.protocol.close()
    text = json.dumps(summary, indent=2)
    print(text)
    if case is not None:
        write_new(case / "summary.json", text.encode("utf-8") + b"\n")
    if not summary["passed"]:
        print(f"windows_smoke: FAILED: {summary['error']}", file=sys.stderr)
        if case is not None:
            for name in ("browser.log", "cef-native.log"):
                print(f"----- tail of {case / name} -----", file=sys.stderr)
                print(tail(case / name), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
