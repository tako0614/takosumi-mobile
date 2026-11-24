#!/usr/bin/env python3
"""Diagnostic-only, self-launched probe for a Windows fixture App crash.

This does not attach to an existing PID, write a dump, or qualify the App.
It starts the explicitly supplied fixture executable under the Windows debug
API, assigns that suspended process to the existing kill-on-close Job before
the debug-event pump runs, and retains only a last-chance 0xC0000005 summary.
The caller must provide a fresh fixture case directory carrying the runner's
`.owned-browser-reference` marker. Only local fixture environment variables
are forwarded; no inherited credentials or WebView2 overrides are passed.

The native API path is intentionally Windows-only and needs native CI
verification. Portable tests exercise event policy, projection and cleanup.
"""
from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path, PureWindowsPath
import re
import subprocess
import sys
import threading
import time
from dataclasses import dataclass


CREATE_SUSPENDED = 0x00000004
DEBUG_ONLY_THIS_PROCESS = 0x00000002
DETACHED_PROCESS = 0x00000008
DEBUG_EVENT_EXCEPTION = 1
DEBUG_EVENT_CREATE_THREAD = 2
DEBUG_EVENT_CREATE_PROCESS = 3
DEBUG_EVENT_EXIT_THREAD = 4
DEBUG_EVENT_EXIT_PROCESS = 5
DEBUG_EVENT_LOAD_DLL = 6
DEBUG_EVENT_UNLOAD_DLL = 7
DEBUG_EVENT_OUTPUT_STRING = 8
DEBUG_EVENT_RIP = 9
EXCEPTION_BREAKPOINT = 0x80000003
EXCEPTION_ACCESS_VIOLATION = 0xC0000005
DBG_CONTINUE = 0x00010002
DBG_EXCEPTION_NOT_HANDLED = 0x80010001
ERROR_SEM_TIMEOUT = 121
WAIT_TIMEOUT_SECONDS = 25
EXIT_WAIT_SECONDS = 5
PROCESS_CASE = re.compile(r"[a-z0-9][a-z0-9-]{0,23}\Z")
MODULE_NAME = re.compile(r"[A-Za-z0-9_.-]{1,96}\Z")
SAFE_SYSTEM_ENV = {
    "APPDATA", "HOMEDRIVE", "HOMEPATH", "LOCALAPPDATA", "PATH", "PROGRAMDATA",
    "SYSTEMROOT", "TEMP", "TMP", "USERPROFILE", "WINDIR",
}
SAFE_FIXTURE_ENV = {
    "TAKOSUMI_BROWSER_PROCESS_APP", "TAKOSUMI_BROWSER_PROCESS_CASE",
    "TAKOSUMI_BROWSER_PROCESS_INSPECT",
}


@dataclass(frozen=True)
class DebugEvent:
    code: int
    process_id: int
    thread_id: int
    exception_code: int | None = None
    first_chance: bool | None = None
    exception_address: int | None = None
    exit_code: int | None = None
    close_handles: tuple[int, ...] = ()
    image_base: int | None = None


def continue_status(event: DebugEvent, target_pid: int, process_created: bool,
                    initial_breakpoint_seen: bool) -> tuple[int, bool]:
    """Return the only accepted initial breakpoint and fail-through policy."""
    if event.code != DEBUG_EVENT_EXCEPTION:
        return DBG_CONTINUE, initial_breakpoint_seen
    if (event.process_id == target_pid and process_created and not initial_breakpoint_seen and
            event.exception_code == EXCEPTION_BREAKPOINT and event.first_chance is True):
        return DBG_CONTINUE, True
    return DBG_EXCEPTION_NOT_HANDLED, initial_breakpoint_seen


def observed_fault(event: DebugEvent, target_pid: int) -> bool:
    """Only the spawned App's last-chance access violation is diagnostic."""
    return (event.process_id == target_pid and event.code == DEBUG_EVENT_EXCEPTION and
            event.exception_code == EXCEPTION_ACCESS_VIOLATION and event.first_chance is False)


def module_projection(module_name: str | None, image_base: int | None,
                      fault_address: int | None) -> dict[str, str | None]:
    """Project a mapped image to basename and relative offset, never an address/path."""
    if not isinstance(module_name, str) or not MODULE_NAME.fullmatch(module_name):
        return {"faultingModule": None, "faultingOffset": None,
                "moduleReason": "module_name_unavailable"}
    if (type(image_base) is not int or type(fault_address) is not int or
            image_base < 0 or fault_address < image_base):
        return {"faultingModule": None, "faultingOffset": None,
                "moduleReason": "module_range_unavailable"}
    return {"faultingModule": module_name,
            "faultingOffset": f"0x{fault_address - image_base:x}",
            "moduleReason": "module_mapped"}


def safe_fixture_environment(source: dict[str, str], case_dir: Path) -> dict[str, str]:
    """Keep OS launch variables and fixed fixture controls; drop all credentials."""
    result = {key: value for key, value in source.items()
              if key.upper() in SAFE_SYSTEM_ENV - {"TEMP", "TMP"} and isinstance(value, str)}
    for key in SAFE_FIXTURE_ENV:
        value = source.get(key)
        if isinstance(value, str):
            result[key] = value
    result["TAKOSUMI_BROWSER_PROCESS_CASE"] = str(case_dir)
    result["TAKOSUMI_BROWSER_PROCESS_INSPECT"] = "1"
    result["TAKOSUMI_BROWSER_PROCESS_APP"] = "local"
    case_tmp = case_dir / "tmp"
    result["TEMP"] = str(case_tmp)
    result["TMP"] = str(case_tmp)
    # CEF child executable, external sites and caller-selected WebView2
    # runtimes are never forwarded by this diagnostic.
    return result


def loopback_site(site_url: str | None) -> str | None:
    """Accept only the Windows fixture's HTTPS loopback origin format."""
    if not isinstance(site_url, str):
        return None
    match = re.fullmatch(r"https://127\.0\.0\.1:([1-9][0-9]{0,4})", site_url)
    if match is None:
        return None
    port = int(match.group(1))
    return site_url if 1 <= port <= 65535 else None


def owned_fixture_case(case_dir: Path) -> bool:
    """Require the exact marker written by windows_browser_process.run()."""
    try:
        if not PROCESS_CASE.fullmatch(case_dir.name) or case_dir.is_symlink() or not case_dir.is_dir():
            return False
        marker = case_dir / ".owned-browser-reference"
        if marker.is_symlink() or not marker.is_file() or marker.stat().st_size > 64:
            return False
        temp_dir = case_dir / "tmp"
        if temp_dir.is_symlink() or not temp_dir.is_dir():
            return False
        return marker.read_text(encoding="utf-8") == case_dir.name
    except (OSError, UnicodeError):
        return False


class ProbeApiUnavailable(RuntimeError):
    pass


class _ExceptionRecord(ctypes.Structure):
    pass


_ULONG_PTR = ctypes.c_size_t
_ExceptionRecord._fields_ = [
    ("ExceptionCode", wintypes.DWORD),
    ("ExceptionFlags", wintypes.DWORD),
    ("ExceptionRecord", ctypes.POINTER(_ExceptionRecord)),
    ("ExceptionAddress", ctypes.c_void_p),
    ("NumberParameters", wintypes.DWORD),
    ("ExceptionInformation", _ULONG_PTR * 15),
]


class _ExceptionDebugInfo(ctypes.Structure):
    _fields_ = [("ExceptionRecord", _ExceptionRecord), ("dwFirstChance", wintypes.DWORD)]


class _CreateThreadDebugInfo(ctypes.Structure):
    _fields_ = [("hThread", wintypes.HANDLE), ("lpThreadLocalBase", ctypes.c_void_p),
                ("lpStartAddress", ctypes.c_void_p)]


class _CreateProcessDebugInfo(ctypes.Structure):
    _fields_ = [("hFile", wintypes.HANDLE), ("hProcess", wintypes.HANDLE),
                ("hThread", wintypes.HANDLE), ("lpBaseOfImage", ctypes.c_void_p),
                ("dwDebugInfoFileOffset", wintypes.DWORD), ("nDebugInfoSize", wintypes.DWORD),
                ("lpThreadLocalBase", ctypes.c_void_p), ("lpStartAddress", ctypes.c_void_p),
                ("lpImageName", ctypes.c_void_p), ("fUnicode", wintypes.WORD)]


class _LoadDllDebugInfo(ctypes.Structure):
    _fields_ = [("hFile", wintypes.HANDLE), ("lpBaseOfDll", ctypes.c_void_p),
                ("dwDebugInfoFileOffset", wintypes.DWORD), ("nDebugInfoSize", wintypes.DWORD),
                ("lpImageName", ctypes.c_void_p), ("fUnicode", wintypes.WORD)]


class _OutputStringDebugInfo(ctypes.Structure):
    _fields_ = [("lpDebugStringData", ctypes.c_void_p), ("fUnicode", wintypes.WORD),
                ("nDebugStringLength", wintypes.WORD)]


class _RipInfo(ctypes.Structure):
    _fields_ = [("dwError", wintypes.DWORD), ("dwType", wintypes.DWORD)]


class _DebugEventPayload(ctypes.Union):
    _fields_ = [("Exception", _ExceptionDebugInfo), ("CreateThread", _CreateThreadDebugInfo),
                ("CreateProcessInfo", _CreateProcessDebugInfo), ("ExitCode", wintypes.DWORD),
                ("LoadDll", _LoadDllDebugInfo), ("UnloadDll", ctypes.c_void_p),
                ("OutputString", _OutputStringDebugInfo), ("RipInfo", _RipInfo)]


class _DebugEvent(ctypes.Structure):
    _fields_ = [("dwDebugEventCode", wintypes.DWORD), ("dwProcessId", wintypes.DWORD),
                ("dwThreadId", wintypes.DWORD), ("u", _DebugEventPayload)]


class _ModuleEntry32W(ctypes.Structure):
    _fields_ = [("dwSize", wintypes.DWORD), ("th32ModuleID", wintypes.DWORD),
                ("th32ProcessID", wintypes.DWORD), ("GlblcntUsage", wintypes.DWORD),
                ("ProccntUsage", wintypes.DWORD), ("modBaseAddr", ctypes.c_void_p),
                ("modBaseSize", wintypes.DWORD), ("hModule", wintypes.HMODULE),
                ("szModule", wintypes.WCHAR * 256), ("szExePath", wintypes.WCHAR * 260)]


_U32 = ctypes.c_uint32
_U64 = ctypes.c_uint64
_HANDLE = wintypes.HANDLE
_POINTER = ctypes.c_void_p
_AMD64 = 0x8664
_CONTEXT_FULL = 0x0010000B
_ADDR_MODE_FLAT = 3
_SYSTEM32 = 0x00000800
_FRAME_LIMIT = 8
_LOCAL_SYMBOL_OPTIONS = (
    0x00000004  # SYMOPT_DEFERRED_LOADS
    | 0x00000080  # SYMOPT_IGNORE_CVREC
    | 0x00001000  # SYMOPT_IGNORE_NT_SYMPATH
    | 0x00080000  # SYMOPT_NO_PROMPTS
    | 0x00200000  # SYMOPT_IGNORE_IMAGEDIR
    | 0x02000000  # SYMOPT_DISABLE_SYMSRV_AUTODETECT
)


class _Address64(ctypes.Structure):
    _fields_ = [("Offset", _U64), ("Segment", ctypes.c_uint16), ("Mode", _U32)]


class _KdHelp64(ctypes.Structure):
    _fields_ = [
        ("Thread", _U64), ("ThCallbackStack", _U32), ("ThCallbackBStore", _U32),
        ("NextCallback", _U32), ("FramePointer", _U32), ("KiCallUserMode", _U64),
        ("KeUserCallbackDispatcher", _U64), ("SystemRangeStart", _U64),
        ("KiUserExceptionDispatcher", _U64), ("StackBase", _U64), ("StackLimit", _U64),
        ("BuildVersion", _U32), ("RetpolineStubFunctionTableSize", _U32),
        ("RetpolineStubFunctionTable", _U64), ("RetpolineStubOffset", _U32),
        ("RetpolineStubSize", _U32), ("Reserved0", _U64 * 2),
    ]


class _StackFrame64(ctypes.Structure):
    _fields_ = [
        ("AddrPC", _Address64), ("AddrReturn", _Address64), ("AddrFrame", _Address64),
        ("AddrStack", _Address64), ("AddrBStore", _Address64), ("FuncTableEntry", _POINTER),
        ("Params", _U64 * 4), ("Far", _U32), ("Virtual", _U32), ("Reserved", _U64 * 3),
        ("KdHelp", _KdHelp64),
    ]


class _Amd64Context(ctypes.Structure):
    # Only registers needed to initialize STACKFRAME64 are named. The Windows
    # API also reads/writes the remaining fixed-size CONTEXT storage.
    _fields_ = [
        ("Home", _U64 * 6), ("ContextFlags", _U32), ("MxCsr", _U32),
        ("Segments", ctypes.c_uint16 * 6), ("EFlags", _U32), ("Debug", _U64 * 6),
        ("IntegerBeforeStack", _U64 * 4), ("Rsp", _U64), ("Rbp", _U64),
        ("IntegerAfterFrame", _U64 * 10), ("Rip", _U64), ("Extended", ctypes.c_byte * 976),
    ]


def _declare(dll, name, restype, *argtypes):
    function = getattr(dll, name)
    function.argtypes = list(argtypes)
    function.restype = restype
    return function


def stopped_amd64_frames(kernel, pid: int, tid: int, exception_address: int, case_dir: Path):
    """Yield only transient PCs after the stopped thread and exception agree."""
    if (ctypes.sizeof(_Amd64Context) != 1232 or _Amd64Context.Rip.offset != 248 or
            ctypes.sizeof(_StackFrame64) != 264 or ctypes.sizeof(_POINTER) != 8):
        return
    open_process = _declare(kernel, "OpenProcess", _HANDLE, _U32, wintypes.BOOL, _U32)
    open_thread = _declare(kernel, "OpenThread", _HANDLE, _U32, wintypes.BOOL, _U32)
    process_of_thread = _declare(kernel, "GetProcessIdOfThread", _U32, _HANDLE)
    wow64 = _declare(kernel, "IsWow64Process2", wintypes.BOOL, _HANDLE,
                    ctypes.POINTER(ctypes.c_uint16), ctypes.POINTER(ctypes.c_uint16))
    get_context = _declare(kernel, "GetThreadContext", wintypes.BOOL, _HANDLE, _POINTER)
    process = open_process(0x0410, False, pid)  # query information and read
    if not process:
        return
    thread = None
    try:
        thread = open_thread(0x0048, False, tid)  # query information and get context
        if not thread or process_of_thread(thread) != pid:
            return
        process_machine, native_machine = ctypes.c_uint16(), ctypes.c_uint16()
        if not wow64(process, ctypes.byref(process_machine), ctypes.byref(native_machine)):
            return
        if process_machine.value != 0 or native_machine.value != _AMD64:
            return
        # Windows needs a 16-byte-aligned CONTEXT, independent of Python FFI alignment.
        memory = ctypes.create_string_buffer(ctypes.sizeof(_Amd64Context) + 15)
        context = _Amd64Context.from_address((ctypes.addressof(memory) + 15) & ~15)
        context.ContextFlags = _CONTEXT_FULL
        if not get_context(thread, ctypes.byref(context)) or context.Rip != exception_address or not context.Rsp:
            return
        yield from _unwind(process, thread, context, case_dir)
    finally:
        if thread:
            kernel.CloseHandle(thread)
        kernel.CloseHandle(process)


def _unwind(process, thread, context: _Amd64Context, case_dir: Path):
    search_path = str(case_dir)
    if not re.fullmatch(r"[A-Za-z]:\\[^;*]*", search_path):
        return
    dbghelp = ctypes.WinDLL("dbghelp.dll", winmode=_SYSTEM32)
    initialize = _declare(dbghelp, "SymInitializeW", wintypes.BOOL, _HANDLE, wintypes.LPCWSTR, wintypes.BOOL)
    cleanup = _declare(dbghelp, "SymCleanup", wintypes.BOOL, _HANDLE)
    options = _declare(dbghelp, "SymSetOptions", _U32, _U32)
    table_access = _declare(dbghelp, "SymFunctionTableAccess64", _POINTER, _HANDLE, _U64)
    module_base = _declare(dbghelp, "SymGetModuleBase64", _U64, _HANDLE, _U64)
    walk = _declare(dbghelp, "StackWalk64", wintypes.BOOL, _U32, _HANDLE, _HANDLE,
                    ctypes.POINTER(_StackFrame64), *([_POINTER] * 5))
    # Local PE unwind data only. No symbol servers, external paths or user prompts.
    options(_LOCAL_SYMBOL_OPTIONS)
    if not initialize(process, search_path, True):
        return
    try:
        frame = _StackFrame64()
        frame.AddrPC = _Address64(context.Rip, 0, _ADDR_MODE_FLAT)
        frame.AddrFrame = _Address64(context.Rbp or context.Rsp, 0, _ADDR_MODE_FLAT)
        frame.AddrStack = _Address64(context.Rsp, 0, _ADDR_MODE_FLAT)
        seen = {(frame.AddrPC.Offset, frame.AddrStack.Offset)}
        for step in range(_FRAME_LIMIT + 1):
            if not walk(_AMD64, process, thread, ctypes.byref(frame), ctypes.byref(context), None,
                        ctypes.cast(table_access, _POINTER), ctypes.cast(module_base, _POINTER), None):
                break
            pc, sp = frame.AddrPC.Offset, frame.AddrStack.Offset
            if not pc or not sp:
                break
            if (pc, sp) in seen:
                # Some versions return the initial context as their first step.
                if step == 0:
                    continue
                break
            seen.add((pc, sp))
            yield pc
            if len(seen) > _FRAME_LIMIT:
                break
    finally:
        cleanup(process)


def caller_frame_projection(api, event: DebugEvent, case_dir: Path,
                            app_image_base: int | None) -> list[dict[str, object]]:
    """Project only mapped module-relative frames. Missing unwind keeps the fault."""
    if event.exception_address is None:
        return []
    try:
        addresses = list(stopped_amd64_frames(api.kernel32, event.process_id, event.thread_id,
                                               event.exception_address, case_dir))
        frames = []
        for address in addresses:
            name, base = api.module_for_address(event.process_id, address)
            projected = module_projection(name, base, address)
            module, offset = projected["faultingModule"], projected["faultingOffset"]
            if module is not None and offset is not None:
                frame: dict[str, object] = {"module": module, "offset": offset}
                if app_image_base and base == app_image_base:
                    frame["fixtureAppImage"] = True
                frames.append(frame)
        return frames
    except Exception:
        return []


class _ImageHlpLine64(ctypes.Structure):
    _fields_ = [("SizeOfStruct", _U32), ("Key", _POINTER), ("LineNumber", _U32),
                ("FileName", wintypes.LPWSTR), ("Address", _U64)]


class _SymbolIndexInfoW(ctypes.Structure):
    # Windows WCHAR and GUID have fixed widths independent of the test host.
    _fields_ = [("sizeofstruct", _U32), ("file", ctypes.c_uint16 * 261),
                ("stripped", _U32), ("timestamp", _U32), ("size", _U32),
                ("dbgfile", ctypes.c_uint16 * 261), ("pdbfile", ctypes.c_uint16 * 261),
                ("guid", _U32 * 4), ("sig", _U32), ("age", _U32)]


def local_symbol_index_status(dbghelp, binary: Path, pdbs: list[Path]) -> str:
    """Compare local PDB indexes without retaining identifiers or paths."""
    if ctypes.sizeof(_SymbolIndexInfoW) != 1608:
        return "unreadable"
    try:
        get_index = _declare(dbghelp, "SymSrvGetFileIndexInfoW", wintypes.BOOL,
                             wintypes.LPCWSTR, ctypes.POINTER(_SymbolIndexInfoW), _U32)
    except AttributeError:
        return "unreadable"

    def index(path):
        info = _SymbolIndexInfoW()
        info.sizeofstruct = ctypes.sizeof(info)
        if get_index(str(path), ctypes.byref(info), 0) and any(info.guid) and not info.sig and info.age:
            return tuple(info.guid), info.age
        return None

    expected = index(binary)
    if expected is None:
        return "unreadable"
    candidates = [index(path) for path in pdbs]
    if expected in candidates:
        return "matched"
    return "unreadable" if None in candidates else "mismatched"


def source_location(file_name: str | None, line: int, repo: PureWindowsPath) -> dict[str, object] | None:
    """Retain checkout-relative and public Rust paths only."""
    if not file_name or len(file_name) > 1024 or not 1 <= line <= 200_000:
        return None
    path = PureWindowsPath(file_name)
    if path.suffix.lower() != ".rs" or ".." in path.parts:
        return None
    if path.is_relative_to(repo):
        parts = path.relative_to(repo).parts
    elif not path.anchor and path.parts[:1] == ("src-tauri",):
        parts = path.parts
    elif (path.anchor == "\\" and path.parts[1:2] == ("rustc",) and
          len(path.parts) >= 6 and re.fullmatch(r"[a-f0-9]{40}", path.parts[2]) and
          path.parts[3] == "library"):
        # rustc's virtual sysroot, with its public revision for source lookup.
        parts = path.parts[1:]
    else:
        lowered = tuple(part.lower() for part in path.parts)
        try:
            index = lowered.index(".cargo")
        except ValueError:
            return None
        if len(lowered) < index + 7 or lowered[index + 1:index + 3] != ("registry", "src"):
            return None
        if not re.fullmatch(r"index\.crates\.io-[0-9a-f]{8,20}", lowered[index + 3]):
            return None
        if not re.fullmatch(r"[a-z0-9][a-z0-9_.-]*-[0-9][a-z0-9_.-]*", lowered[index + 4]):
            return None
        parts = path.parts[index + 4:]
    if not parts or any(not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", part) for part in parts):
        return None
    return {"file": "/".join(parts), "line": line}


def application_source_lines(frames: list[dict[str, object]], binary: Path, repo: PureWindowsPath,
                             evidence: dict[str, object]) -> None:
    """Read matched line tables from the exact fixture EXE after the child exited."""
    evidence["stage"] = "platform"
    if os.name != "nt" or ctypes.sizeof(_ImageHlpLine64) != 40:
        return
    evidence["stage"] = "local_sidecar"
    search_path = str(binary.parent)
    if not re.fullmatch(r"[A-Za-z]:\\[^;*]*", search_path):
        return
    names = (binary.with_suffix(".pdb"), binary.with_name(binary.stem.replace("-", "_") + ".pdb"))
    pdbs = [path for path in dict.fromkeys(names) if not path.is_symlink() and path.is_file()]
    evidence["pdbCandidates"] = len(pdbs)
    if not pdbs:
        return
    evidence["stage"] = "symbol_index"
    dbghelp = ctypes.WinDLL("dbghelp.dll", winmode=_SYSTEM32)
    evidence["pdbIndex"] = local_symbol_index_status(dbghelp, binary, pdbs)
    evidence["stage"] = "initialize"
    kernel = ctypes.WinDLL("kernel32", winmode=_SYSTEM32)
    current_process = _declare(kernel, "GetCurrentProcess", _HANDLE)
    initialize = _declare(dbghelp, "SymInitializeW", wintypes.BOOL, _HANDLE, wintypes.LPCWSTR, wintypes.BOOL)
    cleanup = _declare(dbghelp, "SymCleanup", wintypes.BOOL, _HANDLE)
    options = _declare(dbghelp, "SymSetOptions", _U32, _U32)
    load = _declare(dbghelp, "SymLoadModuleExW", _U64, _HANDLE, _HANDLE, wintypes.LPCWSTR,
                    wintypes.LPCWSTR, _U64, _U32, _POINTER, _U32)
    get_line = _declare(dbghelp, "SymGetLineFromAddrW64", wintypes.BOOL, _HANDLE, _U64,
                        ctypes.POINTER(_U32), ctypes.POINTER(_ImageHlpLine64))
    # Request line tables and exact matching of the local image's PDB.
    options(_LOCAL_SYMBOL_OPTIONS | 0x00000010 | 0x00000400)
    process = current_process()
    if not initialize(process, search_path, False):
        return
    try:
        evidence["stage"] = "load_module"
        base = load(process, None, str(binary), None, 0, 0, None, 0)
        if not base:
            return
        evidence["stage"] = "find_line"
        lookups = found = address_matches = projected = 0
        for frame in frames:
            module = frame.get("module")
            if (frame.get("fixtureAppImage") is not True or not isinstance(module, str)
                    or module.lower() != binary.name.lower()):
                continue
            offset = frame.get("offset")
            if not isinstance(offset, str) or not re.fullmatch(r"0x[0-9a-f]{1,8}", offset):
                continue
            rva = int(offset, 16)
            if rva == 0:
                continue
            # StackWalk reports a return PC. The preceding byte is at the call site.
            address = base + rva - 1
            line = _ImageHlpLine64()
            line.SizeOfStruct = ctypes.sizeof(line)
            displacement = _U32()
            lookups += 1
            if not get_line(process, address, ctypes.byref(displacement), ctypes.byref(line)):
                continue
            found += 1
            if not (base <= line.Address <= address and line.Address + displacement.value == address):
                continue
            address_matches += 1
            location = source_location(line.FileName, line.LineNumber, repo)
            if location is not None:
                frame["source"] = location
                projected += 1
        evidence.update(stage="completed", lookups=lookups, linesFound=found,
                        addressMatches=address_matches, projectedLines=projected)
    finally:
        cleanup(process)


class WindowsDebugApi:
    """Thin, bounded ctypes adapter. Never emits raw Win32 errors or paths."""

    def __init__(self):
        if os.name != "nt":
            raise ProbeApiUnavailable("windows_only")
        if (ctypes.sizeof(ctypes.c_void_p) != 8 or ctypes.sizeof(_ExceptionRecord) != 152 or
                ctypes.sizeof(_ExceptionDebugInfo) != 160 or ctypes.sizeof(_DebugEvent) != 176 or
                ctypes.sizeof(_CreateProcessDebugInfo) != 72 or ctypes.sizeof(_LoadDllDebugInfo) != 40 or
                ctypes.sizeof(_ModuleEntry32W) != 1080):
            raise ProbeApiUnavailable("native_layout_unsupported")
        self.kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        self.kernel32.WaitForDebugEvent.argtypes = [ctypes.POINTER(_DebugEvent), wintypes.DWORD]
        self.kernel32.WaitForDebugEvent.restype = wintypes.BOOL
        self.kernel32.ContinueDebugEvent.argtypes = [wintypes.DWORD, wintypes.DWORD, wintypes.DWORD]
        self.kernel32.ContinueDebugEvent.restype = wintypes.BOOL
        self.kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
        self.kernel32.CloseHandle.restype = wintypes.BOOL
        self.kernel32.CreateToolhelp32Snapshot.argtypes = [wintypes.DWORD, wintypes.DWORD]
        self.kernel32.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
        self.kernel32.Module32FirstW.argtypes = [wintypes.HANDLE, ctypes.POINTER(_ModuleEntry32W)]
        self.kernel32.Module32FirstW.restype = wintypes.BOOL
        self.kernel32.Module32NextW.argtypes = [wintypes.HANDLE, ctypes.POINTER(_ModuleEntry32W)]
        self.kernel32.Module32NextW.restype = wintypes.BOOL

    def wait_for_debug_event(self, timeout_ms: int) -> DebugEvent | None:
        native = _DebugEvent()
        if not self.kernel32.WaitForDebugEvent(ctypes.byref(native), timeout_ms):
            if ctypes.get_last_error() == ERROR_SEM_TIMEOUT:
                return None
            raise ProbeApiUnavailable("debug_event_unavailable")
        code, pid, tid = native.dwDebugEventCode, native.dwProcessId, native.dwThreadId
        handles: tuple[int, ...] = ()
        exception_code = first_chance = address = exit_code = image_base = None
        if code == DEBUG_EVENT_EXCEPTION:
            exception = native.u.Exception
            exception_code = exception.ExceptionRecord.ExceptionCode
            first_chance = bool(exception.dwFirstChance)
            address = int(exception.ExceptionRecord.ExceptionAddress or 0)
        elif code == DEBUG_EVENT_CREATE_PROCESS:
            info = native.u.CreateProcessInfo
            # The API explicitly transfers ownership of hFile. The process
            # and thread event handles are managed by the debug-event lifecycle.
            handle = info.hFile
            handles = (int(handle),) if handle else ()
            image_base = int(info.lpBaseOfImage) if info.lpBaseOfImage else None
        elif code == DEBUG_EVENT_LOAD_DLL:
            handle = native.u.LoadDll.hFile
            handles = (int(handle),) if handle else ()
        elif code == DEBUG_EVENT_EXIT_PROCESS:
            exit_code = native.u.ExitCode
        return DebugEvent(code, pid, tid, exception_code, first_chance, address, exit_code, handles, image_base)

    def continue_debug_event(self, event: DebugEvent, status: int) -> None:
        if not self.kernel32.ContinueDebugEvent(event.process_id, event.thread_id, status):
            raise ProbeApiUnavailable("debug_event_continue_failed")

    def close_event_handles(self, event: DebugEvent) -> None:
        for handle in event.close_handles:
            self.kernel32.CloseHandle(handle)

    def module_for_address(self, pid: int, address: int) -> tuple[str | None, int | None]:
        snapshot = self.kernel32.CreateToolhelp32Snapshot(0x00000008 | 0x00000010, pid)
        invalid = ctypes.c_void_p(-1).value
        if snapshot in (None, invalid):
            return None, None
        try:
            entry = _ModuleEntry32W()
            entry.dwSize = ctypes.sizeof(entry)
            found = self.kernel32.Module32FirstW(snapshot, ctypes.byref(entry))
            while found:
                base = int(entry.modBaseAddr or 0)
                size = int(entry.modBaseSize)
                if base <= address < base + size:
                    # szModule is already a basename; szExePath is deliberately
                    # never returned or written.
                    return entry.szModule, base
                entry.dwSize = ctypes.sizeof(entry)
                found = self.kernel32.Module32NextW(snapshot, ctypes.byref(entry))
            return None, None
        finally:
            self.kernel32.CloseHandle(snapshot)


def _job_factory():
    repo = Path(__file__).resolve().parents[2]
    sys.path.insert(0, str(repo / "native-tests" / "cef-candidate"))
    from windows_smoke import WindowsJob  # noqa: PLC0415
    return WindowsJob()


def run_probe(binary: Path, case_dir: Path, *, timeout_seconds: int = WAIT_TIMEOUT_SECONDS,
              site_url: str, source_env: dict[str, str] | None = None, popen_factory=None,
              job_factory=None, api_factory=None, on_module_observed=None) -> dict[str, object]:
    """Self-launch one fixture process and pump its debug events on this thread."""
    result: dict[str, object] = {"diagnosticComplete": False, "observed": False,
                                 "reason": "probe_unavailable"}
    process = None
    job = None
    api = None
    if ((os.name != "nt" and api_factory is None) or not owned_fixture_case(case_dir) or
            binary.name.lower() != "takosumi-mobile.exe" or
            binary.is_symlink() or not binary.is_file() or type(timeout_seconds) is not int or
            timeout_seconds < 1 or timeout_seconds > 60 or loopback_site(site_url) is None):
        result["reason"] = "fixture_or_platform_unavailable"
        return result
    case_dir = case_dir.resolve()
    binary = binary.resolve()
    create = popen_factory or subprocess.Popen
    job_closed = False
    process_reaped = False
    try:
        job = (job_factory or _job_factory)()
        api = (api_factory or WindowsDebugApi)()
        env = safe_fixture_environment(source_env if source_env is not None else dict(os.environ), case_dir)
        env["TAKOSUMI_BROWSER_PROCESS_SITE"] = site_url
        process = create([str(binary)], cwd=str(case_dir), env=env, stdin=subprocess.DEVNULL,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                         creationflags=CREATE_SUSPENDED | DEBUG_ONLY_THIS_PROCESS | DETACHED_PROCESS)
        # WindowsJob.adopt assigns the suspended exact child to its kill-on-close
        # job before resuming its initial thread. This synchronous function then
        # pumps events on the same thread that called Popen/CreateProcess.
        job.adopt(process.pid)
        deadline = time.monotonic() + timeout_seconds
        created = False
        app_image_base = None
        initial_breakpoint_seen = False
        fault: dict[str, object] | None = None
        while time.monotonic() < deadline:
            wait_ms = max(1, min(1000, int((deadline - time.monotonic()) * 1000)))
            event = api.wait_for_debug_event(wait_ms)
            if event is None:
                continue
            try:
                if event.process_id == process.pid and event.code == DEBUG_EVENT_CREATE_PROCESS:
                    created = True
                    app_image_base = event.image_base
                if fault is None and observed_fault(event, process.pid):
                    module_name, image_base = api.module_for_address(
                        process.pid, event.exception_address or 0)
                    module = module_projection(module_name, image_base, event.exception_address)
                    if on_module_observed and module["moduleReason"] == "module_mapped":
                        on_module_observed(module)
                    fault = {
                        **module,
                        "callerFrames": caller_frame_projection(api, event, case_dir, app_image_base),
                    }
                status, initial_breakpoint_seen = continue_status(
                    event, process.pid, created, initial_breakpoint_seen)
                api.continue_debug_event(event, status)
                if event.process_id == process.pid and event.code == DEBUG_EVENT_EXIT_PROCESS:
                    if fault is None:
                        result = {"diagnosticComplete": True, "observed": False,
                                  "reason": "process_exited_without_target_exception",
                                  "processExitCode": event.exit_code}
                    else:
                        result = {"diagnosticComplete": True, "observed": True,
                                  "reason": "last_chance_access_violation",
                                  "exceptionCode": "0xC0000005", **fault,
                                  "processExitCode": event.exit_code}
                    try:
                        process.wait(timeout=EXIT_WAIT_SECONDS)
                    except Exception:
                        pass
                    return result
            finally:
                try:
                    api.close_event_handles(event)
                except Exception:
                    pass
        if fault is not None:
            result = {"diagnosticComplete": False, "observed": True,
                      "reason": "exception_seen_exit_unconfirmed", "exceptionCode": "0xC0000005",
                      **fault}
        else:
            result = {"diagnosticComplete": False, "observed": False, "reason": "probe_timeout"}
        return result
    except Exception:
        # Do not replace any App verdict with native error text, command lines,
        # or paths. This probe has no authority to report the App as passing.
        result = {"diagnosticComplete": False, "observed": False, "reason": "probe_unavailable"}
        return result
    finally:
        if process is not None and job is not None:
            try:
                if process.poll() is None:
                    job.terminate()
                    if api is not None:
                        _drain_exit_event(api, process.pid, EXIT_WAIT_SECONDS)
            except Exception:
                pass
        if job is not None:
            try:
                job.close()
                job_closed = True
            except Exception:
                job_closed = False
        if process is not None:
            try:
                process.wait(timeout=EXIT_WAIT_SECONDS)
                process_reaped = True
            except Exception:
                try:
                    process.kill()
                    process.wait(timeout=EXIT_WAIT_SECONDS)
                    process_reaped = True
                except Exception:
                    process_reaped = False
        if isinstance(result, dict) and "diagnosticComplete" in result:
            result["jobHandleClosed"] = job_closed
            result["ownedProcessReaped"] = process_reaped


def _drain_exit_event(api, pid: int, timeout_seconds: int) -> bool:
    """Terminate only the owned job, then drain its matching debug exit event."""
    deadline = time.monotonic() + timeout_seconds
    created = False
    breakpoint_seen = False
    while time.monotonic() < deadline:
        timeout_ms = max(1, min(500, int((deadline - time.monotonic()) * 1000)))
        event = api.wait_for_debug_event(timeout_ms)
        if event is None:
            continue
        try:
            if event.process_id == pid and event.code == DEBUG_EVENT_CREATE_PROCESS:
                created = True
            status, breakpoint_seen = continue_status(event, pid, created, breakpoint_seen)
            api.continue_debug_event(event, status)
            if event.process_id == pid and event.code == DEBUG_EVENT_EXIT_PROCESS:
                return True
        finally:
            try:
                api.close_event_handles(event)
            except Exception:
                pass
    return False


def main() -> int:
    parser = argparse.ArgumentParser(description="Diagnostic-only fixture crash probe; never an App pass.")
    parser.add_argument("--binary", required=True, type=Path, help="the fixture-built takosumi-mobile.exe")
    parser.add_argument("--case", required=True, type=Path, help="a fresh owned fixture case directory")
    parser.add_argument("--site", required=True, help="the fixture HTTPS origin https://127.0.0.1:<port>")
    parser.add_argument("--timeout-seconds", type=int, default=WAIT_TIMEOUT_SECONDS)
    args = parser.parse_args()

    def image_identity():
        try:
            if args.binary.is_symlink():
                return None
            info = args.binary.stat()
            return info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns
        except OSError:
            return None

    def publish_module(module):
        # Retain this projection if a later native unwind hits the process deadline.
        print(json.dumps({"moduleObserved": module}, sort_keys=True), flush=True)

    identity = image_identity()
    report = run_probe(args.binary, args.case, site_url=args.site, timeout_seconds=args.timeout_seconds,
                       on_module_observed=publish_module)
    frames = report.get("callerFrames")
    if (isinstance(frames, list) and report.get("diagnosticComplete") is True
            and report.get("ownedProcessReaped") is True
            and any(frame.get("fixtureAppImage") is True for frame in frames)):
        print(json.dumps({"stackObserved": report}, sort_keys=True), flush=True)
        evidence: dict[str, object] = {"stage": "image_identity"}
        try:
            if identity is not None and image_identity() == identity:
                application_source_lines(frames, args.binary.resolve(),
                                         PureWindowsPath(Path(__file__).resolve().parents[2]), evidence)
        except Exception:
            evidence["error"] = True
        if image_identity() != identity:
            for frame in frames:
                frame.pop("source", None)
            evidence = {"stage": "image_identity"}
        report["sourceResolution"] = "available" if any("source" in frame for frame in frames) else "unavailable"
        report["sourceResolutionEvidence"] = evidence
    print(json.dumps(report, sort_keys=True))
    return 0 if report.get("diagnosticComplete") is True else 2


if __name__ == "__main__":
    raise SystemExit(main())
