#!/usr/bin/env python3
"""Windows App package checks (W6): install, launch and uninstall the NSIS bundle.

  python windows_package.py install --installer <setup.exe> --dir <new dir> --manifest <stage.json> --report <json>
  python windows_package.py smoke --exe <installed App> --log <file> --report <json> [--seconds 8]
  python windows_package.py uninstall --dir <install dir> --report <json>

Python 3 standard library only (ctypes for Win32); Windows only, except the
pure helpers its unit tests (test_windows_package.py) cover.

install runs the unsigned per-user installer silently (`/S /D=<dir>`), then
requires the installed `cef\\` to hold exactly the staged files
(scripts/stage-windows-cef.mjs's stage.json: same paths, sizes and SHA-256)
and names the installed App executable.

smoke launches the installed product App (not the fixture) once, on a machine
where it has no runtime root yet, with no
TAKOSUMI_ or WEBVIEW2_ variable, suspended inside a kill-on-close Job Object
this script owns: its top-level window must appear, the App must stay up for
`--seconds`, must not report its browser unavailable (the bundled browser,
guard and runtime root resolved without environment variables) and must have
its runtime root `<app cache>\\cef`. WM_CLOSE to its window must end it with
exit code 0, and the job must then empty; processes left in it are
reported (`leftovers`: pid, image, parent pid, creation time; no command
line).

uninstall requires the installer's per-user uninstall key and Start menu
shortcut for --product to exist, runs its uninstaller silently, waits until
the installation directory, the key and the shortcut are gone and no
process runs from it, and reports what remains of
the App on the machine (its data directories), without removing any of it.

Each command prints one JSON report line and writes it to --report; it never
removes anything besides what the installer and uninstaller do.
"""
import argparse
import ctypes
from datetime import datetime, timezone
import hashlib
import json
import ntpath
import os
import re
import subprocess
import sys
import time
import xml.etree.ElementTree as ET
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[0] / "cef-candidate"))
from windows_smoke import CREATE_SUSPENDED, Failure, WindowsJob, describe_processes, process_records, require  # noqa: E402

WINDOWS = os.name == "nt"
DETACHED_PROCESS = 0x8
WM_CLOSE = 0x0010
GW_OWNER = 4
PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
TH32CS_SNAPPROCESS = 0x2
INSTALL_WAIT = 600
UNINSTALL_WAIT = 180
WINDOW_WAIT = 60
# As windows_browser_process.py: the App's own exit, then its WebView2 and
# any other descendant leaving the job.
APP_EXIT_WAIT = 45
LEFTOVER_WAIT = 10
UNAVAILABLE = "The desktop browser is unavailable"
PRODUCT = "Takosumi"
IDENTIFIER = "com.takosumi"


# --- Pure helpers (unit-tested on any OS) ----------------------------------------

def leftover_message(value):
    """The smoke's failure when processes stayed in the App's job: how many
    and, when they could be read, each one's pid, image, parent and creation
    time."""
    message = f"{value['jobActive']} processes were left in the App's job"
    if value.get("leftovers"):
        return f"{message}: {describe_processes(value['leftovers'])}"
    if value.get("leftoversError"):
        return f"{message} (not described: {value['leftoversError']})"
    return message


def installer_command(installer, directory):
    """NSIS silent install into `directory`. `/D=` must come last and is never
    quoted, so the directory may hold no space or quote."""
    text = str(directory)
    require(" " not in text and '"' not in text, f"The install directory must have no space or quote: {text}")
    return [str(installer), "/S", f"/D={text}"]


def installed_app(names):
    """The App executable among an installation's top-level names: the one
    `.exe` that is not the uninstaller."""
    found = [name for name in names if name.lower().endswith(".exe") and name.lower() != "uninstall.exe"]
    require(len(found) == 1, f"Expected one App executable in the installation, found {sorted(found)}")
    return found[0]


def stage_differences(manifest, installed, size_of, sha256_of):
    """How the installed `cef\\` differs from the stage manifest: missing,
    different (size or SHA-256) and unexpected files, by stage path.
    `installed` lists the installed files' stage paths (`/`-separated)."""
    expected = {entry["path"]: entry for entry in manifest["files"]}
    missing = sorted(path for path in expected if path not in installed)
    unexpected = sorted(path for path in installed if path not in expected)
    different = sorted(path for path, entry in expected.items() if path in installed
                       and (size_of(path) != entry["size"] or sha256_of(path) != entry["sha256"]))
    return {"missing": missing, "different": different, "unexpected": unexpected}


def remains(roots, listed):
    """Where the App's data remains after an uninstall: under each data
    root, the entries `listed(root)` names (the product's and the fixture's
    identifiers)."""
    return [str(Path(root) / name) for root in roots for name in sorted(listed(Path(root)))]


def require_installed_entries(product, key, shortcut):
    """Before an uninstall: the installer's per-user uninstall key and Start
    menu shortcut for `product` must exist, so their absence afterwards means
    the uninstaller removed them."""
    require(key, f"No uninstall key for {product} before the uninstall: the check would pass vacuously")
    require(shortcut, f"No Start menu shortcut for {product} before the uninstall: the check would pass vacuously")


def identified(names):
    """The product's and the fixture's data directory names among `names`."""
    return [name for name in names if name == IDENTIFIER or name.startswith(IDENTIFIER + ".")]


# --- Windows ----------------------------------------------------------------------

def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        while chunk := source.read(1 << 20):
            digest.update(chunk)
    return digest.hexdigest()




def clean_env():
    return {key: value for key, value in os.environ.items()
            if not key.upper().startswith(("TAKOSUMI_", "WEBVIEW2_"))}


def _ntstatus(value, width):
    """A bounded hexadecimal Windows exception/status field, or None."""
    text = str(value or "").strip()
    if text.lower().startswith("0x"):
        text = text[2:]
    if not re.fullmatch(rf"[0-9a-fA-F]{{1,{width}}}", text):
        return None
    return "0x" + text.upper()


def application_error_summary(events, pid, executable):
    """Keep only a same-process Application Error's safe diagnostic fields.

    Event XML and its paths are transient query inputs; neither is returned.
    Application Error 1000 can omit AppPath, so exact PID remains mandatory
    and a present path must match the launched executable.
    """
    if isinstance(events, dict):
        reason = events.get("reason")
        if isinstance(reason, str) and reason in {
                "no_matching_event", "app_path_mismatch", "event_query_limit",
                "event_query_unavailable", "event_query_timeout"}:
            return {"available": False, "reason": reason}
    if not isinstance(events, list):
        return {"available": False, "reason": "event_query_unavailable"}

    expected_path = ntpath.normcase(ntpath.normpath(str(executable)))
    pid_match = False
    for event in events:
        if not isinstance(event, dict):
            continue
        try:
            raw_pid = str(event.get("processId", "")).strip()
            event_pid = int(raw_pid, 16) if raw_pid.lower().startswith("0x") else int(raw_pid, 10)
        except (TypeError, ValueError):
            continue
        if event_pid != pid:
            continue
        pid_match = True
        event_path = str(event.get("appPath") or "").strip()
        if event_path and ntpath.normcase(ntpath.normpath(event_path)) != expected_path:
            continue

        raw_module = str(event.get("faultingModule") or "").strip()
        module = ntpath.basename(raw_module.replace("/", "\\")) or None
        if module is not None and not re.fullmatch(r"[\x20-\x7e]{1,96}", module):
            module = None
        return {
            "available": True,
            "reason": "pid_matched_path_unavailable" if not event_path else "pid_and_path_matched",
            "faultingModule": module,
            "exceptionCode": _ntstatus(event.get("exceptionCode"), 8),
            "faultingOffset": _ntstatus(event.get("faultingOffset"), 16),
        }
    return {"available": False,
            "reason": "app_path_mismatch" if pid_match else "no_matching_event"}


class EventLogUnavailable(Exception):
    """The local Windows Event Log API could not return a result."""


class EventLogTimeout(Exception):
    """The local Event Log query did not complete inside its bound."""


class WindowsEventLogApi:
    """Small ctypes wrapper for the Windows Event Log API used by W6."""

    QUERY_REVERSE_DIRECTION = 0x200
    QUERY_CHANNEL_PATH = 0x1
    RENDER_EVENT_XML = 1
    ERROR_INSUFFICIENT_BUFFER = 122
    ERROR_NO_MORE_ITEMS = 259
    ERROR_TIMEOUT = 1460
    MAX_EVENT_XML_BYTES = 64 * 1024

    def __init__(self):
        from ctypes import wintypes

        self.handle_type = ctypes.c_void_p
        self.dword_type = wintypes.DWORD
        self.library = ctypes.WinDLL("wevtapi.dll", use_last_error=True)
        self.library.EvtQuery.argtypes = [self.handle_type, wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.DWORD]
        self.library.EvtQuery.restype = self.handle_type
        self.library.EvtNext.argtypes = [self.handle_type, wintypes.DWORD,
                                         ctypes.POINTER(self.handle_type), wintypes.DWORD,
                                         wintypes.DWORD, ctypes.POINTER(wintypes.DWORD)]
        self.library.EvtNext.restype = wintypes.BOOL
        self.library.EvtRender.argtypes = [self.handle_type, self.handle_type, wintypes.DWORD,
                                           wintypes.DWORD, wintypes.LPVOID,
                                           ctypes.POINTER(wintypes.DWORD), ctypes.POINTER(wintypes.DWORD)]
        self.library.EvtRender.restype = wintypes.BOOL
        self.library.EvtClose.argtypes = [self.handle_type]
        self.library.EvtClose.restype = wintypes.BOOL

    @staticmethod
    def _last_error():
        return ctypes.get_last_error()

    def query(self):
        xpath = ("*[System[Provider[@Name='Application Error'] and EventID=1000 "
                 "and TimeCreated[timediff(@SystemTime) <= 300000]]]")
        handle = self.library.EvtQuery(None, "Application", xpath,
                                       self.QUERY_CHANNEL_PATH | self.QUERY_REVERSE_DIRECTION)
        if not handle:
            raise EventLogUnavailable()
        return handle

    def next_event(self, query_handle, timeout_ms):
        event = self.handle_type()
        returned = self.dword_type()
        if self.library.EvtNext(query_handle, 1, ctypes.byref(event), timeout_ms, 0,
                                ctypes.byref(returned)):
            return event
        error = self._last_error()
        if error == self.ERROR_NO_MORE_ITEMS:
            return None
        if error == self.ERROR_TIMEOUT:
            raise EventLogTimeout()
        raise EventLogUnavailable()

    def render_xml(self, event_handle):
        used = self.dword_type()
        properties = self.dword_type()
        ok = self.library.EvtRender(None, event_handle, self.RENDER_EVENT_XML, 0, None,
                                    ctypes.byref(used), ctypes.byref(properties))
        error = self._last_error()
        if ok or error != self.ERROR_INSUFFICIENT_BUFFER or used.value == 0:
            raise EventLogUnavailable()
        if used.value > self.MAX_EVENT_XML_BYTES:
            raise EventLogUnavailable()
        chars = used.value // ctypes.sizeof(ctypes.c_wchar) + 1
        buffer = ctypes.create_unicode_buffer(chars)
        if not self.library.EvtRender(None, event_handle, self.RENDER_EVENT_XML, used.value,
                                     buffer, ctypes.byref(used), ctypes.byref(properties)):
            raise EventLogUnavailable()
        return buffer.value

    def close(self, handle):
        if handle:
            self.library.EvtClose(handle)


def event_fields(xml_text):
    """Extract only the fields needed to match and summarize one event."""
    root = ET.fromstring(xml_text)
    namespace = {"e": "http://schemas.microsoft.com/win/2004/08/events/event"}
    provider = root.find("./e:System/e:Provider", namespace)
    event_id = root.findtext("./e:System/e:EventID", namespaces=namespace)
    created = root.find("./e:System/e:TimeCreated", namespace)
    system_time = created.get("SystemTime") if created is not None else None
    if provider is None or provider.get("Name") != "Application Error" or event_id != "1000" or not system_time:
        return None
    values = {item.get("Name"): item.text or "" for item in root.findall("./e:EventData/e:Data", namespace)
              if item.get("Name")}
    values["systemTime"] = system_time
    return values


def _utc_datetime(value):
    return value.replace(tzinfo=timezone.utc) if value.tzinfo is None else value.astimezone(timezone.utc)


def read_application_error(api, pid, executable, started_at, maximum=64):
    """Read a bounded number of recent, exact-process Application Error events."""
    query_handle = None
    path_mismatch = False
    try:
        query_handle = api.query()
        for _ in range(maximum):
            event_handle = api.next_event(query_handle, 200)
            if event_handle is None:
                break
            try:
                try:
                    fields = event_fields(api.render_xml(event_handle))
                except (ET.ParseError, EventLogUnavailable, OSError, ValueError):
                    continue
                if fields is None:
                    continue
                try:
                    event_time = _utc_datetime(datetime.fromisoformat(fields["systemTime"].replace("Z", "+00:00")))
                    launch_time = _utc_datetime(started_at)
                except (TypeError, ValueError):
                    continue
                if event_time < launch_time:
                    continue
                event = {
                    "processId": fields.get("ProcessId", ""),
                    "appPath": fields.get("AppPath", ""),
                    "faultingModule": fields.get("ModuleName", ""),
                    "exceptionCode": fields.get("ExceptionCode", ""),
                    "faultingOffset": fields.get("FaultingOffset", ""),
                }
                summary = application_error_summary([event], pid, executable)
                if summary.get("reason") == "no_matching_event":
                    continue
                if summary.get("reason") == "app_path_mismatch":
                    path_mismatch = True
                    continue
                if summary.get("available"):
                    return summary
            finally:
                try:
                    api.close(event_handle)
                except OSError:
                    pass
        else:
            return {"available": False, "reason": "event_query_limit"}
        return {"available": False,
                "reason": "app_path_mismatch" if path_mismatch else "no_matching_event"}
    except EventLogTimeout:
        return {"available": False, "reason": "event_query_timeout"}
    except (EventLogUnavailable, OSError):
        return {"available": False, "reason": "event_query_unavailable"}
    finally:
        if query_handle is not None:
            try:
                api.close(query_handle)
            except OSError:
                pass


def _application_error_worker():
    """Child entry point: raw event XML and executable paths never leave it."""
    try:
        payload = json.load(sys.stdin)
        started_at = datetime.fromisoformat(payload["startedAt"].replace("Z", "+00:00"))
        summary = read_application_error(WindowsEventLogApi(), int(payload["pid"]),
                                         str(payload["executable"]), started_at)
    except (AttributeError, KeyError, TypeError, ValueError, EventLogTimeout, EventLogUnavailable, OSError):
        summary = {"available": False, "reason": "event_query_unavailable"}
    print(json.dumps(summary, separators=(",", ":")), flush=True)
    return 0


def application_error_event(pid, executable, started_at):
    """Best-effort bounded native Event Log read, isolated behind a hard timeout."""
    payload = json.dumps({"pid": pid, "executable": str(executable), "startedAt": started_at.isoformat()})
    try:
        result = subprocess.run(
            [sys.executable, "-B", str(Path(__file__).resolve()), "_application-error-worker"],
            input=payload, capture_output=True, text=True, timeout=2, env=clean_env(), check=False)
    except subprocess.TimeoutExpired:
        return {"available": False, "reason": "event_query_timeout"}
    except (OSError, UnicodeError):
        return {"available": False, "reason": "event_query_unavailable"}
    if result.returncode != 0:
        return {"available": False, "reason": "event_query_unavailable"}
    try:
        summary = json.loads(result.stdout)
    except (TypeError, ValueError):
        return {"available": False, "reason": "event_query_unavailable"}
    if isinstance(summary, dict) and summary.get("available") is False:
        reason = summary.get("reason")
        if isinstance(reason, str) and reason in {
                "no_matching_event", "app_path_mismatch", "event_query_limit",
                "event_query_unavailable", "event_query_timeout"}:
            return {"available": False, "reason": reason}
    if isinstance(summary, dict) and summary.get("available") is True:
        reason = summary.get("reason")
        if isinstance(reason, str) and reason in {"pid_and_path_matched", "pid_matched_path_unavailable"}:
            module = summary.get("faultingModule")
            if module is not None:
                module = ntpath.basename(module.replace("/", "\\")) if isinstance(module, str) else None
                if module is not None and not re.fullmatch(r"[\x20-\x7e]{1,96}", module):
                    module = None
            return {"available": True, "reason": reason, "faultingModule": module,
                    "exceptionCode": _ntstatus(summary.get("exceptionCode"), 8),
                    "faultingOffset": _ntstatus(summary.get("faultingOffset"), 16)}
    return {"available": False, "reason": "event_query_unavailable"}


def require_app_running(app, value, executable, started_at, message_suffix):
    """Preserve the smoke verdict while adding a bounded diagnostic on crash."""
    exit_code = app.poll()
    if exit_code is not None:
        if exit_code != 0:
            value["applicationError"] = application_error_event(app.pid, executable, started_at)
        require(False, f"The App exited with {exit_code} {message_suffix}")


def user32():
    from ctypes import wintypes
    library = ctypes.WinDLL("user32", use_last_error=True)
    library.EnumWindows.argtypes = [ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM),
                                    wintypes.LPARAM]
    library.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    library.GetWindowThreadProcessId.restype = wintypes.DWORD
    library.IsWindowVisible.argtypes = [wintypes.HWND]
    library.GetWindow.argtypes = [wintypes.HWND, ctypes.c_uint]
    library.GetWindow.restype = wintypes.HWND
    library.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
    library.PostMessageW.argtypes = [wintypes.HWND, ctypes.c_uint, wintypes.WPARAM, wintypes.LPARAM]
    return library


def app_windows(library, pid):
    """The visible, unowned top-level windows of process `pid`: (handle, title)."""
    from ctypes import wintypes
    found = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def visit(window, _):
        owner = wintypes.DWORD()
        library.GetWindowThreadProcessId(window, ctypes.byref(owner))
        if owner.value == pid and library.IsWindowVisible(window) and not library.GetWindow(window, GW_OWNER):
            title = ctypes.create_unicode_buffer(512)
            library.GetWindowTextW(window, title, 512)
            found.append((window, title.value))
        return True

    library.EnumWindows(visit, 0)
    return found


def processes_under(directory):
    """Running processes whose executable lies in `directory`: (pid, image)."""
    from ctypes import wintypes

    class Entry(ctypes.Structure):  # PROCESSENTRY32W
        _fields_ = [("dwSize", wintypes.DWORD), ("cntUsage", wintypes.DWORD), ("th32ProcessID", wintypes.DWORD),
                    ("th32DefaultHeapID", ctypes.c_size_t), ("th32ModuleID", wintypes.DWORD),
                    ("cntThreads", wintypes.DWORD), ("th32ParentProcessID", wintypes.DWORD),
                    ("pcPriClassBase", ctypes.c_long), ("dwFlags", wintypes.DWORD),
                    ("szExeFile", ctypes.c_wchar * 260)]

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
    kernel32.OpenProcess.restype = wintypes.HANDLE
    kernel32.QueryFullProcessImageNameW.argtypes = [wintypes.HANDLE, wintypes.DWORD, wintypes.LPWSTR,
                                                    ctypes.POINTER(wintypes.DWORD)]
    prefix = os.path.normcase(str(directory)).rstrip("\\") + "\\"
    snapshot = kernel32.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
    require(snapshot and snapshot != wintypes.HANDLE(-1).value, "CreateToolhelp32Snapshot failed")
    found = []
    try:
        entry = Entry(dwSize=ctypes.sizeof(Entry))
        more = kernel32.Process32FirstW(snapshot, ctypes.byref(entry))
        while more:
            process = kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, entry.th32ProcessID)
            if process:
                try:
                    image = ctypes.create_unicode_buffer(32768)
                    size = wintypes.DWORD(32768)
                    if kernel32.QueryFullProcessImageNameW(process, 0, image, ctypes.byref(size)) \
                            and os.path.normcase(image.value).startswith(prefix):
                        found.append((entry.th32ProcessID, image.value))
                finally:
                    kernel32.CloseHandle(process)
            more = kernel32.Process32NextW(snapshot, ctypes.byref(entry))
    finally:
        kernel32.CloseHandle(snapshot)
    return found


def install(args, value):
    directory = Path(os.path.abspath(args.dir))
    require(not os.path.lexists(directory), f"{directory} already exists; install into a new directory")
    command = installer_command(Path(os.path.realpath(args.installer)), directory)
    started = time.monotonic()
    code = subprocess.run(command, timeout=INSTALL_WAIT, check=False).returncode
    require(code == 0, f"The installer exited with {code}")
    require(directory.is_dir(), f"The installer did not create {directory}")
    names = sorted(entry.name for entry in os.scandir(directory))
    app = installed_app(names)
    require((directory / "uninstall.exe").is_file(), "The installation has no uninstall.exe")
    cef = directory / "cef"
    require(cef.is_dir(), "The installation has no cef directory")
    installed = {path.relative_to(cef).as_posix() for path in cef.rglob("*") if path.is_file()}
    with open(args.manifest, encoding="utf-8") as source:
        manifest = json.load(source)
    differences = stage_differences(manifest, installed, lambda path: (cef / path).stat().st_size,
                                    lambda path: sha256(cef / path))
    value.update(installed=str(directory), app=app, top=names, cefFiles=len(installed),
                 fixture=manifest.get("fixture"), differences=differences,
                 seconds=round(time.monotonic() - started, 1))
    require(not any(differences.values()), f"The installed cef directory differs from the stage: {differences}")


def smoke(args, value):
    exe = Path(os.path.realpath(args.exe))
    require(exe.is_file(), f"No App at {exe}")
    runtime = Path(os.environ["LOCALAPPDATA"]) / args.identifier / "cef"
    value.update(app=str(exe), runtimeRootBefore=runtime.is_dir())
    # A fresh machine for this App: the runtime root it creates is its own.
    require(not value["runtimeRootBefore"], f"The runtime root {runtime} existed before the App started")
    library = user32()
    with open(args.log, "xb") as log:
        job = WindowsJob()
        started_at = datetime.now(timezone.utc)
        app = subprocess.Popen([str(exe)], cwd=str(exe.parent), env=clean_env(), stdin=subprocess.DEVNULL,
                               stdout=log, stderr=subprocess.STDOUT,
                               creationflags=CREATE_SUSPENDED | DETACHED_PROCESS)
        try:
            job.adopt(app.pid)
            started = time.monotonic()
            windows = []
            while not windows:
                require_app_running(app, value, exe, started_at,
                                    "before its window appeared")
                require(time.monotonic() - started < WINDOW_WAIT, f"No App window within {WINDOW_WAIT} s")
                time.sleep(0.2)
                windows = app_windows(library, app.pid)
            value["windowSeconds"] = round(time.monotonic() - started, 1)
            value["windows"] = [title for _, title in windows]
            up = time.monotonic()
            while time.monotonic() - up < args.seconds:
                require_app_running(app, value, exe, started_at,
                                    f"after {round(time.monotonic() - up, 1)} s up")
                time.sleep(0.2)
            value["upSeconds"] = args.seconds
            text = Path(args.log).read_bytes().decode("utf-8", "replace")
            value["browserUnavailable"] = UNAVAILABLE in text
            value["runtimeRoot"] = runtime.is_dir()
            # The main window (the one titled like the product, else the first).
            window = next((handle for handle, title in windows if title == PRODUCT), windows[0][0])
            require(library.PostMessageW(window, WM_CLOSE, 0, 0), "WM_CLOSE could not be posted")
            value["exit"] = app.wait(timeout=APP_EXIT_WAIT)
            deadline = time.monotonic() + LEFTOVER_WAIT
            while job.processes()[1] and time.monotonic() < deadline:
                time.sleep(0.25)
            value["jobObserved"], value["jobActive"] = job.processes()
            if value["jobActive"]:
                # Diagnostics only: what was left (image, parent, creation
                # time; no command line).
                try:
                    value["leftovers"] = process_records(job.kernel32, job.process_ids())
                except (OSError, ValueError) as error:
                    value["leftoversError"] = str(error)
        finally:
            if app.poll() is None:
                app.terminate()
                app.wait(timeout=10)
            job.close()
    require(not value["browserUnavailable"], f"The App reported its browser unavailable (see {args.log})")
    require(value["runtimeRoot"], f"The App did not create its runtime root {runtime}")
    require(value["exit"] == 0, f"The App exited with {value['exit']} after WM_CLOSE")
    require(value["jobActive"] == 0, leftover_message(value))


def uninstall(args, value):
    directory = Path(os.path.abspath(args.dir))
    uninstaller = directory / "uninstall.exe"
    require(uninstaller.is_file(), f"No uninstaller at {uninstaller}")
    running = processes_under(directory)
    require(not running, f"Processes still run from the installation: {running}")
    local, roaming = Path(os.environ["LOCALAPPDATA"]), Path(os.environ["APPDATA"])
    shortcut = roaming / "Microsoft/Windows/Start Menu/Programs" / f"{args.product}.lnk"
    value.update(product=args.product, uninstallKeyBefore=uninstall_key(args.product),
                 startMenuShortcutBefore=shortcut.exists())
    require_installed_entries(args.product, value["uninstallKeyBefore"], value["startMenuShortcutBefore"])
    started = time.monotonic()
    # The NSIS uninstaller copies itself to the temp directory and runs from
    # there, so its own process ends at once: wait for its work instead.
    code = subprocess.run([str(uninstaller), "/S"], timeout=UNINSTALL_WAIT, check=False).returncode
    # The directory goes first; its uninstall key and Start menu shortcut a
    # moment later (run 42351529951 read them right after the directory).
    while (os.path.lexists(directory) or uninstall_key(args.product) or shortcut.exists()) \
            and time.monotonic() - started < UNINSTALL_WAIT:
        time.sleep(0.5)
    left = sorted(str(path.relative_to(directory)) for path in directory.rglob("*")) if directory.exists() else []
    value.update(uninstallerExit=code, seconds=round(time.monotonic() - started, 1),
                 installationGone=not os.path.lexists(directory), left=left[:50],
                 processesFromInstallation=[image for _, image in processes_under(directory)],
                 dataRemaining=remains([local, roaming], lambda root: identified(
                     [entry.name for entry in os.scandir(root)]) if root.is_dir() else []),
                 uninstallKey=uninstall_key(args.product),
                 startMenuShortcut=shortcut.exists())
    require(code == 0, f"The uninstaller exited with {code}")
    require(value["installationGone"], f"The installation directory remains: {left[:20]}")
    require(not value["processesFromInstallation"], "Processes still run from the removed installation")
    require(not value["uninstallKey"], f"The uninstall key of {args.product} remains")
    require(not value["startMenuShortcut"], f"The Start menu shortcut {shortcut} remains")


def uninstall_key(product):
    """Whether the per-user uninstall registry entry of `product` exists."""
    import winreg
    try:
        winreg.CloseKey(winreg.OpenKey(winreg.HKEY_CURRENT_USER,
                                       rf"Software\Microsoft\Windows\CurrentVersion\Uninstall\{product}"))
        return True
    except OSError:
        return False


def main():
    for stream in (sys.stdout, sys.stderr):
        stream.reconfigure(errors="backslashreplace")
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    commands = parser.add_subparsers(dest="command", required=True)
    installing = commands.add_parser("install", help="silent per-user install, then the stage comparison")
    installing.add_argument("--installer", required=True)
    installing.add_argument("--dir", required=True, help="a new directory with no space in its path")
    installing.add_argument("--manifest", required=True, help="the stage.json the installer was built from")
    installing.add_argument("--report", required=True)
    launching = commands.add_parser("smoke", help="launch the installed product App once and close it")
    launching.add_argument("--exe", required=True)
    launching.add_argument("--log", required=True, help="a new file for the App's output")
    launching.add_argument("--report", required=True)
    launching.add_argument("--seconds", type=float, default=8.0, help="how long the App must stay up")
    launching.add_argument("--identifier", default=IDENTIFIER, help="the App's identifier (its data directories)")
    removing = commands.add_parser("uninstall", help="silent uninstall, then what remains")
    removing.add_argument("--dir", required=True)
    removing.add_argument("--report", required=True)
    removing.add_argument("--product", required=True,
                          help="the installer's productName, as its Tauri config gives it (the uninstall key and "
                               "the Start menu shortcut, which must exist before and be gone after)")
    args = parser.parse_args()
    if not WINDOWS:
        parser.error("windows_package.py runs on Windows")
    value = {"command": args.command, "passed": False}
    try:
        {"install": install, "smoke": smoke, "uninstall": uninstall}[args.command](args, value)
        value["passed"] = True
    except (Failure, subprocess.TimeoutExpired, OSError) as error:
        value["error"] = f"{type(error).__name__}: {error}"
    # The report, whatever happened.
    print(json.dumps(value, separators=(",", ":")), flush=True)
    with open(args.report, "x", encoding="utf-8") as output:
        output.write(json.dumps(value, indent=2) + "\n")
    if not value["passed"]:
        print(f"windows_package {args.command}: FAILED: {value['error']}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    if len(sys.argv) == 2 and sys.argv[1] == "_application-error-worker":
        sys.exit(_application_error_worker())
    sys.exit(main())
