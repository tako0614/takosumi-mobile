#!/usr/bin/env python3
"""Actual Tauri App on Windows -> per-profile CEF browser processes (W3).

The Windows counterpart of browser_process.py. Python 3 standard library only
(ctypes for Win32, ssl and http.server for the fixture site); no OS tools
besides git, which it reads the source revision from.

The App is built with the feature native-browser-process-fixture, the browser
with the cef-candidate features fixture-inspection and windows-sandbox (W7:
a fixture stage of scripts/stage-windows-cef.mjs, CEF's bootstrap.exe as
takosumi-cef-browser-process.exe with takosumi-cef-browser-process.dll beside
it; the unsandboxed candidate executable built with windows-unsandboxed-candidate
also runs, with --unsandboxed-candidate), and the guard is takosumi-browser-guard.exe.
Each phase starts the App suspended, puts it into a kill-on-close Job Object
this runner owns and resumes it, so the App, its WebView2, its guards and every
browser process are accounted; it then runs windows_browser_observer.py
against the App's CEF browsers and requires a normal App exit, every guard's
normal receipt and an empty job. The observer also requires each profile's
browser to show Chromium's sandbox (W7; the runner checks its records again
and keeps them in each phase's summary), unless --unsandboxed-candidate.
The phases share one case (the App's data):
the first launch, then with --restart and --reset the later launches.

The App's own home (WebView2) is driven in-process by the fixture
(src-tauri/tests/desktop/browser_process.rs); the observer reaches the CEF
pages through the browsers' PID-attested loopback inspection ports and the
windows through Win32. The connections are this runner's local HTTPS fixture
site (https://127.0.0.1:<port>: a synthetic sign-in stand-in, an app home and
an app page); its certificate comes from windows_fixture_tls.ps1, whose CA
the machine must trust. Real sites, notifications, the native file chooser,
popups and the open-outside choice are inspected by the Linux runner only.
External links are never opened: the fixture build records them to the
session's external-open.log instead of calling ShellExecuteW (the record's
shape is unit-tested in the plugin). No case chooses to open a link outside,
and the run requires that log to stay empty (no-opener-request).

With --real-site (W4: excalidraw or photopea) the app home links to that real
third-party site instead of /app/, and only the first launch runs: both
profiles open the site through the home, the observer requires its
readiness, separates synthetic markers, reopens Work and quits in order,
and the cases are named <site>/<case>. A manual evidence run
(.github/workflows/windows-real-sites.yml), not part of CI; anonymous only,
no credential is ever typed.

With --packaged (W6) --binary is an installed fixture App (built with
src-tauri/tauri.windows-cef-fixture.conf.json and installed from its NSIS
installer): no --browser, --guard or runtime override is given, so the App
resolves the bundled browser and guard in its installation's cef directory
and its runtime root in its app cache's cef directory itself, and the same
cases run against it.

It prints `PASS case <name>` for each verified case and writes summary.json
into the case directory, which it never removes.
"""
import argparse
import hashlib
import http.server
import json
import os
import re
import shutil
import ssl
import stat
import subprocess
import sys
import threading
import time
from http.cookies import SimpleCookie
from pathlib import Path
from urllib.parse import parse_qs

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(REPO / "native-tests" / "cef-candidate"))
from windows_smoke import CREATE_SUSPENDED, Failure, WindowsJob, browser_shutdown, require, tail  # noqa: E402
from windows_browser_observer import REAL_SITES  # noqa: E402

WINDOWS = os.name == "nt"
OBSERVER = HERE / "windows_browser_observer.py"
# As browser_process.py: the App gives a browser that agreed to stop up to
# STOP_FINISH (30 s, process_adapter.rs) to end normally, and the runner waits
# that long plus a margin before calling an App exit abnormal.
APP_STOP_FINISH = 30
APP_EXIT_WAIT = APP_STOP_FINISH + 15
OBSERVER_WAIT = 85
# W4 (--real-site): both profiles load a third-party site before the
# observer's first checkpoint, and the App's own bounds grow to match
# (TAKOSUMI_BROWSER_PROCESS_REAL_SITE): its first checkpoint waits 450 s
# (browser_process.rs) and its watchdog 540 s (browser_process_windows.rs);
# the observer must end before that watchdog.
REAL_SITE_OBSERVER_WAIT = 530
# After the App exited: its WebView2 processes and any other descendant must
# leave the job within this bound (windows_smoke.py's leftover bound).
LEFTOVER_WAIT = 10
DETACHED_PROCESS = 0x8
# The case name also names the App's data (see browser_process_windows.rs);
# short, since Chromium's profile files there come close to 260 characters.
CASE_NAME = re.compile(r"[a-z0-9][a-z0-9-]{0,23}")
IDENTIFIER = "com.takosumi.fixture.browserprocess"
# Cases each launch's observer verifies, in order; the runner then requires
# the App's normal exit, every guard's normal receipt and an empty job.
OBSERVED_CASES = {
    "": ("separate-profiles", "home-by-window-switch", "reuse-raises-window", "quit-cancel-stops",
         "quit-partial-order"),
    "restart": ("restart-keeps-separation",),
    "reset": ("clear-only-one-profile",),
}
# Verified by the runner: the first launch's final quit closed every
# connection normally; after all launches nothing was left in a job and no
# link was handed to the App's opener.
QUIT_CASE = "quit-completes"
FINAL_CASES = ("no-process-left", "no-opener-request")
# The first launch's cases whose beforeunload was cancelled: by the observer,
# or on a real site by the page or browser first (the observation's
# dialogClosedBy). Printed as one DIALOG line each.
DIALOG_CASES = ("quit-cancel-stops", "quit-partial-order")
# The Windows guard's forced-cleanup diagnostic (guard_windows_identity.rs):
# one line in the profile's browser.log (the guard's standard error) whenever
# its drain reached the termination, naming what it forced by image file name
# and Chromium process type. The log is the profile's, appended to by every
# launch, so each launch keeps only its own guards' lines (`guardPid`).
GUARD_DIAGNOSTIC = "takosumi-browser-guard: forced-cleanup "
BROWSER_LOG_TAIL = 4 * 1024 * 1024
# The observer's evidence printed to the CI log (bindings and samples stay in
# browser-observation.json).
EVIDENCE_KEYS = ("stage", "error", "failureForeground", "visibility", "zOrder", "sandbox")
WINDOWS_SOURCES = (
    "bun.lock", "package.json", "index.html", "vite.config.ts", "tsconfig.json",
    "native-tests/desktop/windows_browser_process.py", "native-tests/desktop/windows_browser_observer.py",
    "native-tests/desktop/windows_fixture_tls.ps1", "native-tests/cef-candidate/windows_smoke.py",
    "native-tests/cef-candidate/Cargo.toml", "native-tests/cef-candidate/Cargo.lock",
    "native-tests/cef-candidate/build.rs", "native-tests/cef-candidate/.cargo/config.toml",
    "native-tests/cef-candidate/src/process_main.rs", "native-tests/cef-candidate/src/browser_main.rs",
    "native-tests/cef-candidate/src/browser_dll.rs",
    "native-tests/cef-candidate/src/guard_entry.rs", "native-tests/cef-candidate/src/guard_windows.rs",
    "native-tests/cef-candidate/src/guard_windows_identity.rs", "native-tests/cef-candidate/src/process_type.rs",
    "native-tests/cef-candidate/windows/takosumi-cef-browser-process.exe.manifest",
    "src-tauri/Cargo.toml", "src-tauri/Cargo.lock", "src-tauri/build.rs", "src-tauri/src/lib.rs",
    "src-tauri/src/main.rs", "src-tauri/tests/desktop/browser_process.rs",
    "src-tauri/tests/desktop/browser_process_windows.rs", "src-tauri/tauri.conf.json",
    "src-tauri/plugins/app-screen/Cargo.toml", "src-tauri/plugins/app-screen/build.rs",
)
SYNTHETIC_USERS = ("work", "personal")


class Site(http.server.BaseHTTPRequestHandler):
    """The connections' site: a synthetic sign-in stand-in (no credential, a
    name posted to /session sets an HttpOnly session cookie), the signed-in
    app home and its app page. Every page names its role and user in
    <main data-page data-user>."""
    timeout = 15

    def send(self, body, status=200, headers=None):
        data = body.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(data)

    def user(self):
        session = SimpleCookie(self.headers.get("Cookie", "")).get("fixture_session")
        return session.value if session and session.value in SYNTHETIC_USERS else ""

    def do_GET(self):
        path, user = self.path.split("?", 1)[0], self.user()
        if path == "/" and not user:
            self.send("""<!doctype html><title>Sign in</title><main data-page="login">
                <p>Synthetic fixture only; no real credentials.</p>
                <form method="post" action="/session"><label>User <input name="user"></label>
                <button>Sign in</button></form></main>""")
        elif path == "/":
            self.send(f"""<!doctype html><title>{user} apps</title>
                <main data-page="home" data-user="{user}"><h1>{user} apps</h1>
                <a data-app="editor" href="{getattr(self.server, 'app_url', '/app/')}">Editor</a></main>""")
        elif path == "/app/" and user:
            self.send(f"""<!doctype html><title>Editor</title>
                <main data-page="app" data-user="{user}"><h1>Editor</h1><textarea></textarea></main>""")
        elif path == "/app/":
            self.send("", status=303, headers={"Location": "/"})
        else:
            self.send("", status=404)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0") or "0")
        if self.path != "/session" or length > 1024:
            self.send("", status=404)
            return
        user = parse_qs(self.rfile.read(length).decode("utf-8", "replace")).get("user", [""])[0]
        if user not in SYNTHETIC_USERS:
            self.send("", status=400)
            return
        self.send("", status=303, headers={
            "Location": "/",
            "Set-Cookie": f"fixture_session={user}; Path=/; Max-Age=86400; Secure; HttpOnly; SameSite=Lax",
        })

    def log_message(self, format, *args):
        requests = self.server.requests
        if len(requests) < 512:
            requests.append(f"{self.command} {self.path.split('?', 1)[0]} {args[1] if len(args) > 1 else ''}")


def start_site(cert, key, app_url="/app/"):
    """The fixture site on 127.0.0.1 (an ephemeral port), served from a thread
    for the whole run, so every phase reaches the saved connection's origin.
    Its app home links to `app_url`: its own /app/, or (W4) a real site."""
    tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    tls.load_cert_chain(cert, key)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Site)
    server.requests = []
    server.app_url = app_url
    # The handshake runs in each request's thread (on its first read), so a
    # stalled client cannot hold up the accepting thread.
    server.socket = tls.wrap_socket(server.socket, server_side=True, do_handshake_on_connect=False)
    threading.Thread(target=server.serve_forever, name="fixture-site", daemon=True).start()
    return server, f"https://127.0.0.1:{server.server_port}"


def check_site(site, ca):
    """The site answers with a certificate the machine's trust store accepts
    (Python's default context reads the Windows ROOT store), and with the
    fixture CA alone; Chromium verifies against the same store."""
    import urllib.request
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}),
                                         urllib.request.HTTPSHandler(context=ssl.create_default_context()))
    with opener.open(site + "/", timeout=10) as response:
        require(b'data-page="login"' in response.read(), "The fixture site did not serve its sign-in page")
    # With a cafile, the context trusts that CA only.
    pinned = ssl.create_default_context(cafile=str(ca))
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=pinned))
    with opener.open(site + "/", timeout=10) as response:
        require(response.status == 200, "The fixture CA does not verify the site")


def normal(receipt, what="The guard"):
    """browser_process.py's receipt check, for a guard's receipt or the
    session job's counterpart (`what` names it in the failure)."""
    require(type(receipt["rootExit"]) is int and receipt["rootExit"] == 0 and receipt["complete"] is True,
            f"{what} root must exit normally and completely")
    require(receipt["kernelNoChildren"] is True and receipt["remaining"] == [],
            f"{what} must have no remaining child: {receipt['remaining']}")
    require(type(receipt["forcedCleanup"]) is int and receipt["forcedCleanup"] == 0 and receipt["cleanupErrors"] == [],
            f"{what} must not require forced cleanup")
    require(receipt["error"] is None, f"{what} must not report an error")


def create_exclusive(path, mode="w"):
    """A new file this runner writes: never an existing file."""
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_BINARY", 0) | getattr(os, "O_NOFOLLOW", 0)
    handle = os.open(path, flags, 0o600)
    return os.fdopen(handle, "wb") if mode == "wb" else os.fdopen(handle, "w", encoding="utf-8", newline="\n")


def read_session(path, limit=4 * 1024 * 1024):
    """A file the session wrote, read as data: a plain regular file (no link
    or other reparse point) of at most `limit` bytes."""
    info = os.lstat(path)
    reparse = getattr(info, "st_file_attributes", 0) & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)
    require(stat.S_ISREG(info.st_mode) and not reparse, f"Session file {path} is not a plain file")
    with open(path, "rb") as source:
        data = source.read(limit + 1)
    require(len(data) <= limit, f"Session file {path} is larger than {limit} bytes")
    return data


def read_json(path):
    return json.loads(read_session(path))


def digest(file):
    checksum = hashlib.sha256()
    with open(file, "rb") as source:
        while chunk := source.read(1024 * 1024):
            checksum.update(chunk)
    return checksum.hexdigest()


def source_files(repo):
    tracked = subprocess.check_output(["git", "ls-files", "--", "src", "src-tauri/plugins/app-screen/src",
                                       "src-tauri/capabilities"], cwd=repo, text=True).splitlines()
    selected = set(WINDOWS_SOURCES)
    for name in tracked:
        path = Path(name)
        if ((name.startswith("src/") and path.suffix in {".ts", ".tsx", ".css"}) or
                (name.startswith("src-tauri/plugins/app-screen/src/") and path.suffix == ".rs") or
                (name.startswith("src-tauri/capabilities/") and path.suffix == ".json")):
            selected.add(name)
    result = []
    for name in sorted(selected):
        file = repo / name
        require(file.is_file() and not file.is_symlink() and file.resolve().is_relative_to(repo.resolve()),
                f"Expected a local Windows fixture source: {name}")
        result.append(file)
    return result


def new_case(path):
    """A new case directory inside this user's profile, where the App's
    browser runtime root must lie (process_setup_windows.rs). Its name also
    names the App's data, which must not exist yet."""
    parent = Path(os.path.realpath(path.parent))
    profile = Path(os.path.realpath(os.environ["USERPROFILE"]))
    require(parent.is_dir() and parent.is_relative_to(profile),
            f"--data must be a new directory inside the user's profile {profile}")
    require(CASE_NAME.fullmatch(path.name) is not None,
            "--data must be named with at most 24 lowercase letters, digits and hyphens")
    case = parent / path.name
    require(not os.path.lexists(case), "--data must be a new directory")
    return case


def app_data(name):
    """Where Tauri keeps this case's App data on Windows (its known folders)."""
    identifier = f"{IDENTIFIER}.{name}"
    return [Path(os.environ[variable]) / identifier for variable in ("LOCALAPPDATA", "APPDATA")]


def job_pids(job):
    """Process ids still in the job (JobObjectBasicProcessIdList)."""
    import ctypes

    class IdList(ctypes.Structure):
        _fields_ = [("assigned", ctypes.c_uint32), ("listed", ctypes.c_uint32), ("ids", ctypes.c_size_t * 256)]

    ids = IdList()
    if not job.kernel32.QueryInformationJobObject(job.handle, 3, ctypes.byref(ids), ctypes.sizeof(ids), None):
        return None
    return [ids.ids[index] for index in range(min(ids.listed, 256))]


def session_receipt(job, app_exit):
    """The session's counterpart of the Linux outer guard receipt: the App's
    job must empty within LEFTOVER_WAIT of the App's exit, without force."""
    deadline = time.monotonic() + LEFTOVER_WAIT
    while True:
        total, active = job.processes()
        if active == 0 or time.monotonic() >= deadline:
            break
        time.sleep(0.25)
    remaining = job_pids(job) if active else []
    return {"rootExit": app_exit, "complete": True, "kernelNoChildren": active == 0, "remaining": remaining or [],
            "forcedCleanup": 0, "cleanupErrors": [] if remaining is not None else ["JobProcessListUnavailable"],
            "error": None, "observed": total}


def evidence(session, suffix):
    """The observation's decisive evidence as one compact JSON line: the
    stage, any error, the window and visibility records and each case's
    evidence except the per-profile bindings."""
    try:
        observation = read_json(session / f"browser-observation{suffix}.json")
    except (OSError, ValueError, Failure) as error:
        return json.dumps({"observation": f"unreadable: {error}"})
    digest = {key: observation[key] for key in EVIDENCE_KEYS if key in observation}
    digest["cases"] = {name: value for name, value in observation.get("evidence", {}).items()
                       if name != "separate-profiles"}
    return json.dumps(digest, separators=(",", ":"))


def sandbox_summary(observation, opened, required):
    """W7: each opened profile's sandbox evidence from the observation
    (the browser's integrity level, its renderers' and any problem). With
    `required`, every opened profile must have evidence, all without a
    problem (the observer also requires it)."""
    records = observation.get("sandbox", [])
    found = []
    for process in opened["processes"]:
        mine = [record for record in records if record.get("profile") == process["profile"]]
        if required:
            require(mine, f"No sandbox evidence for the browser of {process['profile']}")
        for record in mine:
            if required:
                require(not record.get("problems"),
                        f"The browser of {process['profile']} does not show Chromium's sandbox: {record['problems']}")
            found.append({"profile": process["profile"],
                          "browser": (record.get("browser") or {}).get("integrityName"),
                          "renderers": [renderer.get("integrityName") for renderer in record.get("renderers", [])],
                          "problems": record.get("problems")})
    return found


def shutdown_summary(records):
    """Per profile of a phase: the guard receipt's rootExit and the
    browser's own shutdown line (children still running after CefShutdown,
    how it ended, its exit code), for summary.json and the CI log."""
    found = []
    for record in records or []:
        receipt = record.get("receipt")
        lines = record.get("browserShutdown") or []
        found.append({"profile": record.get("profile"),
                      "rootExit": receipt.get("rootExit") if isinstance(receipt, dict) else None,
                      "browser": lines[-1] if lines else None})
    return found


def guard_diagnostics(text, guard_pid=None):
    """The guard's forced-cleanup lines in a browser log, parsed: with
    `guard_pid`, only that guard's. One that does not parse is kept as
    `unparsed` (at most 500 characters), since it cannot be attributed."""
    found = []
    for line in text.splitlines():
        index = line.find(GUARD_DIAGNOSTIC)
        if index < 0:
            continue
        payload = line[index + len(GUARD_DIAGNOSTIC):]
        try:
            parsed = json.loads(payload)
        except ValueError:
            found.append({"unparsed": payload[:500]})
            continue
        if guard_pid is None or (isinstance(parsed, dict) and parsed.get("guardPid") == guard_pid):
            found.append(parsed)
    return found


def guard_records(session, suffix):
    """Each browser process of a phase: its profile, its guard's pid and
    receipt, that guard's own forced-cleanup lines from the profile's
    browser.log (`forcedCleanupLines`; the receipt's `forcedCleanup` is the
    count) and, on Windows, that browser's own shutdown line
    (`browserShutdown`, W7: children still running after CefShutdown, how
    it ended, its exit code)."""
    marker = session / (f"app-browser-process{suffix}-open.json" if suffix else "app-browser-process-open.json")
    records = []
    for process in read_json(marker)["processes"]:
        log = tail(Path(process["directory"]) / "browser.log", BROWSER_LOG_TAIL)
        record = {"profile": process["profile"], "guardPid": process["pid"],
                  "forcedCleanupLines": guard_diagnostics(log, process["pid"]),
                  "browserShutdown": browser_shutdown(log, process.get("opened", {}).get("nativePid"))}
        try:
            record["receipt"] = read_json(Path(process["guardReceipt"]))
        except (OSError, ValueError, Failure) as error:
            record["receipt"] = f"unreadable: {error}"
        records.append(record)
    return records


def tails(session, suffix):
    """Logs of a failed phase, for the CI log."""
    print(f"----- evidence{suffix} -----", file=sys.stderr)
    print(evidence(session, suffix), file=sys.stderr)
    print(f"----- browser guards{suffix}: receipt and forced-cleanup diagnostics -----", file=sys.stderr)
    try:
        for record in guard_records(session, suffix):
            print(json.dumps(record, separators=(",", ":")), file=sys.stderr)
    except (OSError, ValueError, KeyError, Failure) as error:
        print(f"<unavailable: {error}>", file=sys.stderr)
    names = [f"app{suffix}.log", f"observer{suffix}.log", f"browser-observation{suffix}.json"]
    for name in names:
        print(f"----- tail of {session / name} -----", file=sys.stderr)
        print(tail(session / name), file=sys.stderr)
    marker = session / (f"app-browser-process{suffix}-open.json" if suffix else "app-browser-process-open.json")
    try:
        processes = read_json(marker)["processes"]
    except (OSError, ValueError, KeyError, Failure):
        return
    for process in processes:
        directory = Path(process["directory"])
        for name in ("browser.log", "cef-native.log"):
            print(f"----- tail of {directory / name} -----", file=sys.stderr)
            print(tail(directory / name, 8192), file=sys.stderr)


def run_phase(args, env, files, before, head, suffix):
    """One App launch on the case's data; returns its proof."""
    session, phase = args.session, suffix[1:]
    proof = {"passed": False, "phase": phase or "first", "sourceHead": head, "artifactsBefore": before}
    execution = {"passed": False}
    observer_env = {key: value for key, value in os.environ.items() if not key.upper().startswith("TAKOSUMI_")}
    with create_exclusive(session / f"app{suffix}.log", "wb") as log:
        job = WindowsJob()
        try:
            # Suspended until it is in the job, detached from any console.
            app = subprocess.Popen([str(args.binary)], cwd=session, env=env, stdin=subprocess.DEVNULL, stdout=log,
                                   stderr=subprocess.STDOUT, creationflags=CREATE_SUSPENDED | DETACHED_PROCESS)
        except BaseException:
            job.close()
            raise
        try:
            job.adopt(app.pid)
        except BaseException:
            app.kill()
            app.wait()
            job.close()
            raise
        proof["appPid"] = app.pid
        try:
            real = ["--real-site", args.real_site] if args.real_site else []
            # A packaged run also requires each guard and browser to run from
            # the installed App's cef directory.
            real += ["--installed", str(args.binary.parent)] if args.packaged else []
            # W7: the observer requires each profile's browser to show
            # Chromium's sandbox, unless this is the unsandboxed candidate.
            real += ["--unsandboxed-candidate"] if args.unsandboxed_candidate else []
            with create_exclusive(session / f"observer{suffix}.log") as observer:
                subprocess.run([sys.executable, "-B", str(OBSERVER), "--data", str(session), "--site", args.site,
                                "--app-pid", str(app.pid), "--phase", phase, *real],
                               env=observer_env, check=True,
                               timeout=REAL_SITE_OBSERVER_WAIT if args.real_site else OBSERVER_WAIT,
                               stdout=observer, stderr=subprocess.STDOUT)
            execution["appExit"] = app.wait(timeout=APP_EXIT_WAIT)
            require(execution["appExit"] == 0, "The App must exit normally")
            require(read_json(session / f"app-browser-process{suffix}-complete.json")["passed"] is True,
                    "The actual App flow must pass")
            observation = read_json(session / f"browser-observation{suffix}.json")
            require(observation["passed"] is True, "The browser observation must pass")
            opened = read_json(session / (f"app-browser-process{suffix}-open.json" if suffix
                                          else "app-browser-process-open.json"))
            proof["sandbox"] = sandbox_summary(observation, opened, not args.unsandboxed_candidate)
            execution["browserGuards"] = []
            for process in opened["processes"]:
                receipt = read_json(Path(process["guardReceipt"]))
                normal(receipt)
                execution["browserGuards"].append(receipt)
            proof["sessionJob"] = session_receipt(job, execution["appExit"])
            normal(proof["sessionJob"], "The App's job")
            execution["passed"] = True
            proof["cases"] = observation.get("cases", [])
            proof["passed"] = True
        except Exception as error:
            proof["error"] = execution["error"] = f"{type(error).__name__}: {error}"
            raise
        finally:
            try:
                if app.poll() is None:
                    # This exact Popen is owned; ending it closes the App's own
                    # job, which ends its guards and browsers (no receipts).
                    app.terminate()
                    app.wait(timeout=10)
                total, active = job.processes()
                proof.setdefault("sessionJob", {"observed": total, "remaining": job_pids(job) or []})
                if active:
                    proof["passed"] = False
                    proof["sessionJob"]["forcedCleanup"] = active
                    job.terminate()
            except Exception as error:
                proof["passed"] = False
                proof["cleanupError"] = str(error)
            finally:
                job.close()
                # Every guard has ended: its receipt and what, if anything,
                # its drain forced (diagnostics only). app-execution.json's
                # browserGuards keeps the receipts a passed launch checked.
                try:
                    proof["guardRecords"] = guard_records(session, suffix)
                except (OSError, ValueError, KeyError, Failure) as error:
                    proof["guardRecordsError"] = f"{type(error).__name__}: {error}"
                try:
                    proof["artifactsAfter"] = {str(file): digest(file) for file in files}
                    proof["sourceHeadAfter"] = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO,
                                                                       text=True).strip()
                    if proof["artifactsAfter"] != before or proof["sourceHeadAfter"] != head:
                        proof["passed"] = False
                        proof.setdefault("error", "Sources or executables changed during the run")
                except Exception as error:
                    proof["passed"] = False
                    proof["artifactError"] = str(error)
                with create_exclusive(session / f"app-execution{suffix}.json") as output:
                    output.write(json.dumps(execution, indent=2) + "\n")
                with create_exclusive(args.data / f"execution{suffix}.json") as output:
                    output.write(json.dumps(proof, indent=2) + "\n")
                if not proof["passed"]:
                    tails(session, suffix)
    return proof


def main():
    # Logs and App messages may hold text the console's code page lacks.
    for stream in (sys.stdout, sys.stderr):
        stream.reconfigure(errors="backslashreplace")
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--binary", type=Path, required=True,
                        help="takosumi-mobile.exe built with --features native-browser-process-fixture")
    parser.add_argument("--browser", type=Path,
                        help="takosumi-cef-browser-process.exe of a fixture stage: CEF's bootstrap with the "
                             "browser DLL built with the cef-candidate features fixture-inspection,windows-sandbox "
                             "beside it (or the windows-unsandboxed-candidate executable), libcef.dll beside it; "
                             "not with --packaged")
    parser.add_argument("--guard", type=Path, help="takosumi-browser-guard.exe; not with --packaged")
    parser.add_argument("--packaged", action="store_true",
                        help="W6: --binary is an installed fixture App (tauri.windows-cef-fixture.conf.json); it "
                             "resolves its bundled browser and guard (cef\\ beside it) and its runtime root "
                             "itself, without the fixture's overrides")
    parser.add_argument("--data", type=Path, required=True,
                        help="a new case directory inside the user's profile, named with at most 24 lowercase "
                             "letters, digits and hyphens (the App's browser runtime root must lie in the profile)")
    parser.add_argument("--tls-cert", type=Path, required=True,
                        help="the fixture site's certificate chain (cert.pem from windows_fixture_tls.ps1)")
    parser.add_argument("--tls-key", type=Path, required=True, help="its private key (key.pem)")
    parser.add_argument("--tls-ca", type=Path, required=True, help="the trusted fixture CA (ca.pem)")
    parser.add_argument("--restart", action="store_true",
                        help="relaunch the App on the same data and verify per-profile state after restart")
    parser.add_argument("--reset", action="store_true",
                        help="relaunch, clear Work's site data from the home, and verify only Work was cleared")
    parser.add_argument("--real-site", choices=sorted(REAL_SITES),
                        help="W4: the app home links to this real site; first launch only, cases named <site>/<case>")
    parser.add_argument("--unsandboxed-candidate", action="store_true",
                        help="diagnostics only: the browser is the windows-unsandboxed-candidate executable; its "
                             "sandbox evidence is recorded, not required (W7)")
    args = parser.parse_args()
    if args.real_site and (args.restart or args.reset):
        parser.error("--real-site runs the first launch only")
    if args.packaged and (args.browser or args.guard):
        parser.error("--packaged resolves the browser and guard from the installed App; omit --browser and --guard")
    if not args.packaged and not (args.browser and args.guard):
        parser.error("--browser and --guard are required unless --packaged")
    if not WINDOWS:
        parser.error("windows_browser_process.py runs on Windows; browser_process.py is the Linux runner")
    if args.packaged:
        args.binary = Path(os.path.realpath(args.binary))
        args.browser, args.guard = packaged_files(args.binary)
    for name in ("binary", "browser", "guard", "tls_cert", "tls_key", "tls_ca"):
        setattr(args, name, Path(os.path.realpath(getattr(args, name))))
        require(getattr(args, name).is_file(), f"No file at {getattr(args, name)}")
    args.data = new_case(Path(os.path.abspath(args.data)))
    stored = [directory for directory in app_data(args.data.name) if os.path.lexists(directory)]
    require(not stored, f"App data for this case name already exists: {stored}")
    summary = {"passed": False, "case": str(args.data), "appData": [str(path) for path in app_data(args.data.name)],
               "cases": [], "phases": [], "realSite": args.real_site, "packaged": args.packaged,
               "unsandboxedCandidate": args.unsandboxed_candidate}
    args.data.mkdir()
    args.data.joinpath(".owned-browser-reference").write_text(args.data.name, encoding="utf-8")
    try:
        run(args, summary)
        summary["passed"] = True
    except Exception as error:
        summary["error"] = f"{type(error).__name__}: {error}"
    finally:
        with create_exclusive(args.data / "summary.json") as output:
            output.write(json.dumps(summary, indent=2) + "\n")
    print(json.dumps({key: summary[key] for key in ("passed", "case", "appData", "cases", "error") if key in summary},
                     indent=2))
    if not summary["passed"]:
        print(f"windows_browser_process: FAILED: {summary['error']}", file=sys.stderr)
        return 1
    return 0


def case_name(args, name):
    """W3 cases by name; a W4 real-site run prefixes its site."""
    return f"{args.real_site}/{name}" if args.real_site else name


def dialog_lines(args, observation):
    """`DIALOG closedBy=<observer|browser> case=<name>` for each of a passed
    first launch's DIALOG_CASES (windows-real-sites.yml warns on `browser`)."""
    lines = []
    for name in DIALOG_CASES:
        closed_by = observation.get("evidence", {}).get(name, {}).get("dialogClosedBy")
        require(closed_by in ("observer", "browser"), f"The {name} evidence names no dialogClosedBy")
        lines.append(f"DIALOG closedBy={closed_by} case={case_name(args, name)}")
    return lines


def real_site_cases(site):
    """The cases a W4 run of `site` prints, in order (the first launch's,
    its final quit and the run's invariants)."""
    return [f"{site}/{name}" for name in list(OBSERVED_CASES[""]) + [QUIT_CASE] + list(FINAL_CASES)]


# The executables a packaged App resolves itself (`cef\` in its install).
PACKAGED_BROWSER = "takosumi-cef-browser-process.exe"
PACKAGED_GUARD = "takosumi-browser-guard.exe"


def packaged_files(binary):
    """The browser and guard an installed App resolves without environment
    variables: the cef directory beside its executable (Tauri's resource directory on
    Windows is the executable's)."""
    cef = binary.parent / "cef"
    return cef / PACKAGED_BROWSER, cef / PACKAGED_GUARD


def browser_code(browser):
    """The sandboxed browser's code beside its executable (W7): the DLL
    CEF's bootstrap loads, named like the executable; none for the
    unsandboxed candidate, whose code is the executable itself."""
    code = browser.with_suffix(".dll")
    return [code] if code.is_file() else []


def browser_env(base, args, session, log):
    """The App's environment: the caller's without any TAKOSUMI_ or
    WEBVIEW2_ variable, then the fixture's. A packaged run (`--packaged`)
    names no executable or runtime root: the App resolves its bundled
    browser and guard and its runtime root (its app cache's cef directory)
    itself."""
    env = {key: value for key, value in base.items() if not key.upper().startswith(("TAKOSUMI_", "WEBVIEW2_"))}
    env.update(TEMP=str(session / "tmp"), TMP=str(session / "tmp"),
               TAKOSUMI_BROWSER_PROCESS_CASE=str(session),
               TAKOSUMI_BROWSER_PROCESS_INSPECT="1",
               TAKOSUMI_BROWSER_PROCESS_APP="local",
               TAKOSUMI_BROWSER_PROCESS_SITE=args.site,
               # The fixture build appends any link it would open outside here.
               TAKOSUMI_BROWSER_PROCESS_EXTERNAL_LOG=str(log))
    if not args.packaged:
        env.update(TAKOSUMI_BROWSER_PROCESS_PROGRAM=str(args.browser),
                   TAKOSUMI_BROWSER_PROCESS_GUARD=str(args.guard),
                   TAKOSUMI_BROWSER_PROCESS_RUNTIME=str(session / "r"))
    if args.real_site:
        env["TAKOSUMI_BROWSER_PROCESS_REAL_SITE"] = args.real_site
    return env


def run(args, summary):
    # Everything the App, the observer and the browsers write lives in the
    # session directory, named like the case (its name is the observer's
    # synthetic nonce and the App's data identifier).
    session = args.session = args.data / args.data.name
    session.mkdir()
    session.joinpath(".owned-browser-reference").write_text(args.data.name, encoding="utf-8")
    for name in ("r", "tmp"):
        (session / name).mkdir()
    server, args.site = start_site(args.tls_cert, args.tls_key,
                                   REAL_SITES[args.real_site]["url"] if args.real_site else "/app/")
    summary["site"] = args.site
    try:
        check_site(args.site, args.tls_ca)
        log = session / "external-open.log"
        env = browser_env(os.environ, args, session, log)
        runtime = args.browser.parent / "libcef.dll"
        require(runtime.is_file(), f"No libcef.dll beside {args.browser}")
        files = sorted(set([args.binary, args.browser, runtime, args.guard] + browser_code(args.browser) +
                           source_files(REPO)))
        before = {str(file): digest(file) for file in files}
        snapshot = args.data / "source-snapshot"
        for file in files:
            if file.is_relative_to(REPO) and file.stat().st_size <= 2 * 1024 * 1024:
                target = snapshot / file.relative_to(REPO)
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(file, target)
        head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip()
        phases = [""] + (["-restart"] if args.restart else []) + (["-reset"] if args.reset else [])
        for suffix in phases:
            phase_env = dict(env, TAKOSUMI_BROWSER_PROCESS_PHASE=suffix[1:]) if suffix else env
            proof = run_phase(args, phase_env, files, before, head, suffix)
            shutdown = shutdown_summary(proof.get("guardRecords"))
            summary["phases"].append({"phase": proof["phase"], "passed": proof["passed"],
                                      "sessionJob": proof.get("sessionJob"), "sandbox": proof.get("sandbox"),
                                      "shutdown": shutdown})
            print(f"shutdown{suffix or '-first'} {json.dumps(shutdown, separators=(',', ':'))}", flush=True)
            require(proof["passed"] is True, f"The {proof['phase']} App launch did not pass")
            expected = list(OBSERVED_CASES[suffix[1:]])
            require(proof["cases"] == expected,
                    f"The {proof['phase']} launch verified {proof['cases']}, not {expected}")
            # The first launch's final quit: the App exited 0 after closing
            # Personal (and Work before it) with normal guard receipts.
            print(f"evidence{suffix or '-first'} {evidence(args.session, suffix)}", flush=True)
            for name in expected + ([QUIT_CASE] if not suffix else []):
                print(f"PASS case {case_name(args, name)}", flush=True)
                summary["cases"].append(case_name(args, name))
            if not suffix:
                for line in dialog_lines(args, read_json(args.session / "browser-observation.json")):
                    print(line, flush=True)
        # Every launch ended with its job empty, no forced cleanup and every
        # browser guard's normal receipt (checked per phase above).
        print(f"PASS case {case_name(args, FINAL_CASES[0])}", flush=True)
        summary["cases"].append(case_name(args, FINAL_CASES[0]))
        # No link was handed to the App's opener over all launches: none of
        # these flows chooses to open one outside (that needs the native
        # context menu, inspected on Linux only).
        opened = read_session(log, 1024 * 1024).decode().splitlines() if os.path.lexists(log) else []
        summary["externalOpens"] = len(opened)
        require(opened == [], f"Expected no request to the opener, saw {len(opened)}")
        print(f"PASS case {case_name(args, FINAL_CASES[1])}", flush=True)
        summary["cases"].append(case_name(args, FINAL_CASES[1]))
    finally:
        server.shutdown()
        server.server_close()
        summary["siteRequests"] = server.requests


if __name__ == "__main__":
    sys.exit(main())
