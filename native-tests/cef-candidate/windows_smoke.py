#!/usr/bin/env python3
"""Smoke test of takosumi-cef-browser-process on its own (no guard, no App).

Written for Windows (the windows-latest CI job) and kept OS neutral, so it also
runs against the Linux browser. Python 3 standard library only (ctypes for the
Windows Job Object and window checks); no OS tools.

It creates a new owned case directory and lays it out as the App's adapter
(src-tauri/plugins/app-screen/src/desktop/process_adapter.rs) lays out a
browser's profile directory: `boot.json` holding the first Open request,
`Downloads/`, and on Linux private HOME/XDG/TMPDIR directories. It starts the
browser directly with `TAKOSUMI_BROWSER_PROCESS_DIRECTORY` and the App's
switches: on Windows suspended, inside a kill-on-close Job Object, resumed
once assigned, so every process it starts is accounted; on Linux in its own
session and process group. It then requires, each within a bound:

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
   group (Linux) within 10 s of the exit;
6. with `--expect-sandbox` (Windows, the sandboxed browser of W7), before the
   shutdown: at least one renderer process in the job, each without
   `--no-sandbox` and with a token integrity level of Low or Untrusted,
   below the browser's own (read from its token), and no `--no-sandbox` in
   the browser's command line either; recorded as `sandbox`.

Requests use browser_process::Request's serde form (compact JSON, one line).
It prints one JSON summary on stdout, writes it to `<case>/summary.json` and
exits 1 on any failure, printing the tails of the browser's logs. Leftover
processes are ended (closing the Job Object, or SIGKILL to the group). The
case directory is never removed.
"""
import argparse
import ctypes
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
LIBRARY = "libcef.dll" if WINDOWS else "libcef.so"
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


X64_SIZES = {BasicLimit: 64, IoCounters: 48, ExtendedLimit: 144, Accounting: 48, ThreadEntry: 28}

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


def sandbox_problems(browser, renderers):
    """Why the sandbox is not shown (empty when it is): `browser` and each
    of `renderers` are {"integrity": RID or None, "noSandbox": bool}."""
    problems = []
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
    {"pid", "arguments", "integrity", "error"?}."""
    facts = {"pid": pid, "arguments": None, "integrity": None}
    process = kernel32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not process:
        facts["error"] = f"OpenProcess: Windows error {ctypes.get_last_error()}"
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
    finally:
        kernel32.CloseHandle(process)
    return facts


def sandbox_evidence(job, browser_pid, deadline):
    """Windows: the browser's and its renderers' integrity levels and
    command lines, once at least one renderer is in the job (until
    `deadline`); raises when they do not show Chromium's sandbox."""
    handle, dword = ctypes.c_void_p, ctypes.c_uint32
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
                              "noSandbox": bool(fact.get("arguments")) and has_no_sandbox(fact["arguments"]),
                              **({"error": fact["error"]} if "error" in fact else {})}
    evidence = {"browser": summarize(browser), "renderers": [summarize(fact) for fact in renderers],
                "types": sorted({process_type(fact["arguments"]) or "browser" for fact in facts
                                 if fact["arguments"]})}
    evidence["problems"] = sandbox_problems(evidence["browser"], evidence["renderers"])
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

    def leftovers(self):
        """Processes still owned after the browser exited, waiting up to
        LEFTOVER_WAIT for them to end: ({"total", "active"} on Windows, the
        Job Object's counts; {"groupAlive"} on Linux)."""
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
                        help=f"directory holding {LIBRARY} and the CEF resources (default: the executable's)")
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
                             "low or untrusted, below the browser's; no --no-sandbox)")
    args = parser.parse_args()
    if not (WINDOWS or LINUX):
        parser.error("Only Windows and Linux browsers are supported")
    if args.expect_sandbox and not WINDOWS:
        parser.error("--expect-sandbox reads Windows tokens")
    if not URL_SHAPE.fullmatch(args.url) or args.url.endswith(":443/"):
        parser.error("--url must be a lowercase https site root such as https://example.com/")

    browser = Path(os.path.realpath(args.browser))
    runtime = Path(os.path.realpath(args.runtime or browser.parent))
    summary = {"passed": False, "platform": sys.platform, "browser": str(browser), "runtime": str(runtime),
               "url": args.url, "expectLoad": args.expect_load, "reuse": args.reuse,
               "expectSandbox": args.expect_sandbox, "seconds": {}, "error": None}
    session = None
    case = None
    try:
        require(browser.is_file(), f"No browser executable at {browser}")
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
        else:
            require(len(str(case / "tmp") + SOCKET_SUFFIX) < 108, "Case path too long for Chromium's socket")
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
            summary["sandbox"] = sandbox_evidence(session.job, pid, time.monotonic() + args.open_timeout)
            require(not summary["sandbox"]["problems"],
                    f"The browser's processes do not show Chromium's sandbox: {summary['sandbox']['problems']}")

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
