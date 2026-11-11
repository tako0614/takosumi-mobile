#!/usr/bin/env python3
"""M3: actual WKWebView home -> guarded CEF profiles on macOS 14+.

Reuses the Linux/Windows App flow and the Windows observer's platform-neutral
CDP, dialog and checkpoint helpers. Anonymous Excalidraw or Photopea only;
synthetic storage markers, no login, credential, Keychain change or TLS bypass.
The three launches exercise reuse, OS quit intent/cancellation, restart and
one-profile clear. Page.close drives a browser close on relaunch; OS window
switching, native dialogs/files, notifications and packaging are not qualified.

Each App launch is itself under the existing macOS guard, in addition to the
App's per-profile guards. Only these owned guards receive a stop on failure.
Receipts describe observed/tracked process exhaustion, not Linux ECHILD or
the absence of OS-managed WKWebView services spawned outside the App's tree.
"""
import argparse
import ctypes
import errno
import hashlib
import http.client
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlsplit
import uuid

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "cef-candidate"))
from windows_smoke import Failure, require, seatbelt_evidence  # noqa: E402
from windows_browser_observer import Cdp, FRAME_EVENTS, Observer, Page, REAL_SITES, SETTLE_WAIT, WebSocket  # noqa: E402
from macos_shutdown_sample import same_owner, watch_shutdowns  # noqa: E402
from macos_process_image import PinnedImage  # noqa: E402


class BsdInfo(ctypes.Structure):
    # Darwin proc_bsdinfo, also used by the production macOS adapter/guard.
    _fields_ = [(name, ctypes.c_uint32) for name in (
        "flags", "status", "xstatus", "pid", "parent", "uid", "gid", "ruid",
        "rgid", "svuid", "svgid", "reserved")]
    _fields_ += [("comm", ctypes.c_char * 16), ("name", ctypes.c_char * 32)]
    _fields_ += [(name, ctypes.c_uint32) for name in ("files", "group", "jobc", "tdev", "tpgid")]
    _fields_ += [("nice", ctypes.c_int32), ("seconds", ctypes.c_uint64), ("micros", ctypes.c_uint64)]


class Processes:
    def __init__(self):
        require(sys.platform == "darwin" and ctypes.sizeof(BsdInfo) == 136, "Darwin proc_bsdinfo required")
        self.lib = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
        self.lib.proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64, ctypes.c_void_p, ctypes.c_int]
        self.lib.proc_pidinfo.restype = ctypes.c_int
        self.lib.proc_pidpath.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32]
        self.lib.proc_pidpath.restype = ctypes.c_int

    def facts(self, pid):
        info = BsdInfo()
        ctypes.set_errno(0)
        size = self.lib.proc_pidinfo(pid, 3, 0, ctypes.byref(info), ctypes.sizeof(info))
        if size <= 0 and ctypes.get_errno() in (0, errno.ESRCH, errno.ENOENT):
            return None
        require(size == ctypes.sizeof(info) and info.pid == pid, f"Cannot read process identity {pid}")
        if info.status == 5:  # SZOMB, reaped by its owner / launchd
            return None
        return {"pid": pid, "parent": info.parent, "group": info.group,
                "start": info.seconds * 1_000_000 + info.micros}

    def image(self, pid):
        buffer = ctypes.create_string_buffer(4096)
        require(self.lib.proc_pidpath(pid, buffer, len(buffer)) > 0, f"Cannot read process image {pid}")
        return Path(os.fsdecode(buffer.value)).resolve(strict=True)


def parse_listeners(text, port):
    """lsof -Fpn: only a listener's PID and exact IPv4 loopback address."""
    current, owners = None, []
    for line in text.splitlines():
        if line.startswith("p"):
            require(line[1:].isdigit(), "Malformed lsof PID")
            current = int(line[1:])
        elif line.startswith("n"):
            require(current is not None and line[1:] == f"127.0.0.1:{port}",
                    f"Unexpected inspection listener: {line[:100]}")
            owners.append(current)
    return sorted(set(owners))


def listeners(port):
    require(type(port) is int and 1 <= port <= 65535, "Invalid inspection port")
    result = subprocess.run(["/usr/sbin/lsof", "-nP", f"-iTCP:{port}", "-sTCP:LISTEN", "-Fpn"],
                            capture_output=True, text=True, timeout=5)
    require(result.returncode == 0, f"Cannot identify inspection listener ({result.returncode})")
    return parse_listeners(result.stdout, port)


def normal_receipt(receipt):
    require(receipt.get("rootExit") == 0 and receipt.get("complete") is True
            and receipt.get("kernelNoChildren") is True and receipt.get("forcedCleanup") == 0
            and receipt.get("remaining") == [] and receipt.get("cleanupErrors") == []
            and receipt.get("error") is None, f"Abnormal owned guard receipt: {receipt}")


class MacObserver(Observer):
    def __init__(self, data, site, phase, processes, browser, guard, quit_helper=None):
        super().__init__(data, REAL_SITES[site]["origin"], 0, phase, real_site=site)
        self.processes, self.browser, self.guard = processes, browser, guard
        self.identities = []
        self.quit_helper = quit_helper
        self.proof["scope"] = f"macOS actual App {phase or 'first'}; anonymous {site}, synthetic markers, no login"

    def system_quit(self, page):
        identities = {item["pid"]: item for item in self.identities}
        child, parent = identities[page.native["nativePid"]], identities[page.process["pid"]]
        require(self.quit_helper is not None and same_owner(self.processes, child, parent, self.browser, self.guard),
                "OS quit target is no longer the owned browser")
        result = subprocess.run([str(self.quit_helper), str(child["pid"]), str(parent["pid"]),
                                 str(child["start"]), str(parent["start"])],
                                capture_output=True, text=True, timeout=5, check=False)
        require(result.returncode == 0, f"Normal OS quit request was refused ({result.returncode})")
        require(json.loads(result.stdout) == {"requested": True, "pid": child["pid"]}, "Invalid OS quit receipt")
        # beforeunload is armed: a quit that bypasses it and kills the process
        # must fail here. Acceptance alone cannot satisfy a close/cancel case.
        require(same_owner(self.processes, child, parent, self.browser, self.guard),
                "OS quit target changed before beforeunload was answered")
        self.proof.setdefault("systemQuitRequests", []).append({"pid": child["pid"], "start": child["start"],
                                                                "requested": True, "completionProven": False})

    def attach(self, process):
        native = process["opened"]
        self.step("attach", None)
        guard = self.processes.facts(process["pid"])
        child = self.processes.facts(native["nativePid"])
        require(guard and child and guard["parent"] == self.app_pid
                and child["parent"] == guard["pid"] and child["start"] >= guard["start"],
                "Browser must be its guard's current child")
        require(self.browser.matches(self.processes.image(child["pid"]))
                and self.guard.matches(self.processes.image(guard["pid"])), "Unexpected browser or guard executable")
        require(self.processes.facts(child["pid"]) == child and self.processes.facts(guard["pid"]) == guard,
                "Browser or guard identity changed during image check")
        self.identities.extend([guard, child])
        inspection = json.loads((Path(process["directory"]) / "inspection.json").read_text())
        require(inspection["pid"] == child["pid"] and listeners(inspection["port"]) == [child["pid"]],
                "Only the attested native browser may own the inspection listener")
        require(self.processes.facts(child["pid"]) == child and self.processes.facts(guard["pid"]) == guard,
                "Browser or guard identity changed during listener check")
        port = inspection["port"]
        connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
        try:
            connection.request("GET", "/json/version")
            response = connection.getresponse()
            require(response.status == 200, "Inspection version refused")
            endpoint = urlsplit(json.loads(response.read(65536))["webSocketDebuggerUrl"])
        finally:
            connection.close()
        require(endpoint.scheme == "ws" and endpoint.hostname == "127.0.0.1" and endpoint.port == port,
                "Unexpected inspection endpoint")
        client = Cdp(WebSocket("127.0.0.1", port, endpoint.path), process["profile"])
        self.clients.append(client)
        targets = [item for item in client.command("Target.getTargets")["targetInfos"] if item["type"] == "page"]
        require(len(targets) == 1, "Exactly one page per profile")
        session = client.command("Target.attachToTarget", {"targetId": targets[0]["targetId"], "flatten": True})
        page = Page(client, session["sessionId"], process, targets[0])
        page.send("Page.enable")
        on_site = f"location.origin==={json.dumps(self.app_origin)} && document.readyState==='complete'"
        landing = self.real.get("landing", "false")
        self.site_settled(page, f"{on_site} && ({self.real['ready']} || {landing})", "Anonymous app did not load")
        if page.evaluate(landing):
            page.evaluate(self.real["enter"], True)
        self.site_settled(page, f"{on_site} && {self.real['ready']}", "Anonymous editor did not become ready")
        require(page.evaluate("isSecureContext && !window.__TAURI_INTERNALS__ && !window.__TAURI__"),
                "Remote app must be secure and have no App IPC bridge")
        sandbox = seatbelt_evidence(child["pid"], time.monotonic() + 15)
        self.proof.setdefault("sandbox", []).append(dict(sandbox, profile=process["profile"]))
        require(not sandbox["problems"], f"Renderer sandbox is not proven: {sandbox['problems']}")
        return page

    def sample(self, page):
        key = json.dumps(self.key)
        return page.evaluate(f"({{document:globalThis[{key}]??null,storage:localStorage.getItem({key}),"
                             f"cookie:{self.cookie_of()},origin:location.origin}})")

    def beforeunload(self, page):
        self.step("beforeunload setup", page)
        mark = page.cdp.mark()
        state = page.evaluate("(()=>{window.__sumiBeforeUnload=e=>{e.preventDefault();e.returnValue='';};"
                              "addEventListener('beforeunload',window.__sumiBeforeUnload);"
                              "return {stickyActivation:navigator.userActivation.hasBeenActive,"
                              "active:navigator.userActivation.isActive,visibility:document.visibilityState,"
                              "focus:document.hasFocus(),origin:location.origin}})()", True)
        self.proof.setdefault("beforeUnloadSetup", []).append({"profile": page.process["profile"],
                                                               "unixMicros": time.time_ns() // 1000,
                                                               "state": state})
        return mark

    def checkpoint(self, name):
        self.proof.setdefault("checkpointTimes", []).append({"name": name, "unixMicros": time.time_ns() // 1000})
        super().checkpoint(name)

    def dialog(self, page, mark, kind, what, bound=SETTLE_WAIT):
        self.step(f"{kind} observation", page)
        try:
            return super().dialog(page, mark, kind, what, bound)
        except Failure as error:
            # Keep events/target/receipt before the owned failure cleanup.
            # This never answers a dialog or changes its acceptance/bound.
            self.dialog_failure(what, [(page, mark)], error)
            self.dialog_native_state(page)
            raise

    def busy_dialog(self, page, mark, kind, what, bound=SETTLE_WAIT):
        # A real site's ad iframes can keep the renderer churning past the
        # bound after the quit request is delivered and beforeunload is armed
        # (run 42808568119: 155 frame events plus a Target.getTargets CDP
        # timeout inside the window). While the profile's guard has not ended
        # the dialog is still pending rather than refused, so allow one more
        # bounded window on the same mark. If the profile already exited, the
        # quit skipped the dialog and the original failure stands.
        try:
            return self.dialog(page, mark, kind, what, bound)
        except Failure:
            require(not self.guard_ended(page.process["guardReceipt"]),
                    f"{what} and the profile had already exited")
            return self.dialog(page, mark, kind, f"{what} after the busy-renderer window", 30)

    def quiet_renderer(self, page, quiet=4.0, bound=30):
        # The storm does not always break inside the dialog wait itself (run
        # 42843941116: 179/195 frame events across both 30 s windows, native
        # alive and guard pending the whole time). Ask for the quit only
        # after one frame-silent window so the beforeunload task can be
        # scheduled first — bounded, and never skipped: if the storm never
        # breaks the request goes out on time anyway and the unchanged
        # dialog bound still measures the real result.
        start = time.monotonic()
        end = start + bound
        mark = page.cdp.mark()
        arrived = False
        while time.monotonic() < end:
            page.cdp.drain(quiet)
            if not any(event["method"] in FRAME_EVENTS for event in page.cdp.since(mark)):
                arrived = True
                break
            mark = page.cdp.mark()
        self.proof.setdefault("quietRenderer", []).append(
            {"profile": page.process["profile"], "quietSeconds": quiet,
             "waitedSeconds": round(time.monotonic() - start, 1), "arrived": arrived})

    def dialog_native_state(self, page):
        # After a failed dialog step, record whether the owned native browser
        # is even alive and the tail of its own log. Distinguishes a quit
        # that never reached CEF's close path (busy runloop, queued terminate)
        # from a dialog the loaded page itself suppressed.
        entry = {}
        try:
            entry = self.proof["dialogFailures"][-1]["pages"][-1]
            native = page.native
            try:
                os.kill(native["nativePid"], 0)
                entry["nativeAlive"] = True
            except OSError:
                entry["nativeAlive"] = False
            # page.native is the flattened opened record (pid/window/cache);
            # the profile directory that owns cef-native.log is the parent's.
            log = Path(page.process["directory"]) / "cef-native.log"
            if log.is_file():
                size = log.stat().st_size
                with log.open("rb") as stream:
                    stream.seek(max(0, size - 8 * 1024))
                    entry["nativeLogTail"] = stream.read().decode("utf-8", "replace")[-4000:]
        except Exception as failure:  # diagnostics only; keep the original failure
            entry["nativeStateError"] = repr(failure)[:200]

    @staticmethod
    def remove_beforeunload(page):
        page.evaluate("(()=>{removeEventListener('beforeunload',window.__sumiBeforeUnload);"
                      "delete window.__sumiBeforeUnload;return true})()")

    def first_launch(self):
        opened = self.proof["opened"] = self.marker("app-browser-process-open.json")
        pages = [self.attach(self.process_of(opened, name)) for name in ("Work", "Personal")]
        work, personal = pages
        require(work.native["nativePid"] != personal.native["nativePid"] and work.window != personal.window,
                "Profiles must use different browser processes and windows")
        for index, page in enumerate(pages):
            require(self.sample(page)["storage"] is None and self.sample(page)["cookie"] is None,
                    "Fresh profile already has this case's synthetic state")
            key, value = json.dumps(self.key), json.dumps(f"{index}:{self.nonce}")
            page.evaluate(f"(()=>{{globalThis[{key}]={value};localStorage.setItem({key},{value});"
                          f"document.cookie={key}+'='+{value}+'; Path=/; Max-Age=86400; Secure; SameSite=Lax';return true}})()")
        expected = [self.sample(page) for page in pages]
        require([item["storage"] for item in expected] == [f"0:{self.nonce}", f"1:{self.nonce}"]
                and [item["cookie"] for item in expected] == [f"0:{self.nonce}", f"1:{self.nonce}"],
                "Both profiles must read only their own synthetic state")
        self.case("separate-profiles", expected)
        self.checkpoint("inspect-ready")
        resumed = self.proof["resumed"] = self.marker("app-browser-process.json")
        require(len(resumed["after"]) == 2 and [self.sample(page) for page in pages] == expected
                and work.pages() == [work.target["targetId"]], "Home reuse replaced a document or its state")
        require(self.raise_refusals(work) == [], "Browser refused to raise the reused window")
        self.case("home-reuses-document")
        mark, other = self.beforeunload(work), personal.cdp.mark()
        self.checkpoint("request-exit")
        self.quiet_renderer(work)
        self.system_quit(work)
        dialog = self.busy_dialog(work, mark, "beforeunload", "Work beforeunload did not open")
        personal.cdp.drain()
        require(not any(event["method"].startswith("Page.javascriptDialog") for event in personal.cdp.since(other))
                and not self.guard_ended(personal.process["guardReceipt"]), "Quit reached Personal before Work agreed")
        cancelled_by = self.cancel_dialog(work, mark, "Work cancel was not observed", dialog)
        self.checkpoint("cancel-inspected")
        require(len(self.marker("app-browser-process-blocked.json")) == 2, "Cancelled quit lost a profile")
        self.marker("app-browser-process-cancelled.json")
        require([self.sample(page) for page in pages] == expected, "Cancelled quit changed a document")
        self.remove_beforeunload(work)
        self.case("quit-cancel-stops", {"cancelledBy": cancelled_by})
        mark = self.beforeunload(personal)
        with watch_shutdowns(self, [work]):
            self.checkpoint("request-partial-exit")
            self.quiet_renderer(personal)
            self.system_quit(personal)
            dialog = self.busy_dialog(personal, mark, "beforeunload", "Personal beforeunload did not open", 30)
            receipt = self.normal_receipt(work.process["guardReceipt"], "Work did not close normally before Personal")
        cancelled_by = self.cancel_dialog(personal, mark, "Personal cancel was not observed", dialog)
        self.checkpoint("partial-inspected")
        partial = self.marker("app-browser-process-partial.json")
        require([item["profile"] for item in partial] == [personal.process["profile"]]
                and self.sample(personal) == expected[1], "Partial quit did not retain the same Personal document")
        self.remove_beforeunload(personal)
        self.case("quit-partial-order", {"workReceipt": receipt, "cancelledBy": cancelled_by})
        # The opted-in browser holds Personal's permission prompt for the
        # App's own answer: the driver clicks the home's row before the final
        # quit, and the page's resolved grant is the round-trip evidence.
        self.answer_permission(personal)
        with watch_shutdowns(self, [personal]):
            self.checkpoint("request-final-exit")
            self.finish_final_quit([personal])
        self.proof["passed"] = True

    def relaunch(self, kind):
        opened = self.proof["opened"] = self.marker(f"app-browser-process-{kind}-open.json")
        pages = [self.attach(self.process_of(opened, name)) for name in ("Work", "Personal")]
        samples = [self.sample(page) for page in pages]
        expected = [None if kind == "reset" else f"0:{self.nonce}", f"1:{self.nonce}"]
        require([sample["storage"] for sample in samples] == expected
                and [sample["cookie"] for sample in samples] == expected
                and all(sample["document"] is None for sample in samples), "Restart/reset did not preserve separation")
        if kind == "reset":
            personal = pages[1]
            lock = os.readlink(Path(personal.process["directory"]) / "cef-profiles" / "SingletonLock")
            require(opened["runningClearRefused"] is True and opened["personalSingleton"] == lock
                    and lock.endswith(f"-{personal.native['nativePid']}"), "Running Personal was not kept locked")
        self.case("clear-only-one-profile" if kind == "reset" else "restart-keeps-separation", samples)
        self.checkpoint(f"{kind}-inspected")
        with watch_shutdowns(self, pages):
            try:
                pages[1].send("Page.close")
            except Failure as error:
                # A close may end CDP before replying; only a normal guard receipt
                # below can establish that it closed, never the socket error alone.
                self.proof["pageCloseReplyError"] = str(error)

            self.finish_final_quit([pages[1]])
            self.normal_receipt(pages[1].process["guardReceipt"], "Personal did not close normally")
            self.checkpoint(f"{kind}-request-exit")
            self.finish_final_quit([pages[0]])
        self.proof["passed"] = True

    def failure_capture(self):
        # No foreign process/window inspection or screenshot. Keep only our identities.
        self.proof["identities"] = self.identities


def digest(path):
    with open(path, "rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def preserve_browser_logs(case, phase):
    """Only text logs in this random case's App data; never browser storage."""
    store = bytes(json.loads((case / "home-store.json").read_text()))
    identifier = f"com.takosumi.fixture.browserprocess.m3{store.hex()}"
    root = Path.home() / "Library/Application Support" / identifier / "browser-process-experiment"
    kept = []
    if not root.is_dir():
        return kept
    for profile in root.iterdir():
        try:
            uuid.UUID(profile.name)
        except ValueError:
            continue
        if profile.is_symlink() or not profile.is_dir():
            continue
        for name in ("browser.log", "cef-native.log"):
            source = profile / name
            if source.is_symlink() or not source.is_file():
                continue
            out = case / "evidence" / phase / profile.name / name
            out.parent.mkdir(parents=True, exist_ok=True)
            with source.open("rb") as stream:
                size = source.stat().st_size
                stream.seek(max(0, size - 4 * 1024 * 1024))
                out.write_bytes(stream.read(4 * 1024 * 1024))
            kept.append({"path": str(out.relative_to(case)), "sourceBytes": size, "keptBytes": out.stat().st_size})
    return kept


def run(args):
    processes = Processes()
    binary, browser, guard = (path.resolve(strict=True) for path in (args.binary, args.browser, args.guard))
    # Keep these open across all launches and sampling threads, before any
    # owned process starts. A later replacement cannot redefine the identity.
    with PinnedImage(binary) as app_image, PinnedImage(browser) as browser_image, PinnedImage(guard) as guard_image:
        return run_pinned(args, processes, binary, browser, guard, app_image, browser_image, guard_image)


def run_pinned(args, processes, binary, browser, guard, app_image, browser_image, guard_image):
    case = Path(tempfile.mkdtemp(prefix="sumi-m3-", dir=args.work.resolve(strict=True)))
    (case / ".owned-browser-reference").write_text("macOS actual App fixture\n")
    (case / "home-store.json").write_text(json.dumps(list(uuid.uuid4().bytes)) + "\n")
    runtime = case / "rt"
    runtime.mkdir(mode=0o700)
    proof = {"passed": False, "scope": __doc__, "site": args.site, "case": str(case), "phases": [],
             "source": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=HERE, text=True).strip(),
             "executables": {str(path): digest(path) for path in (binary, browser, guard)}}
    print(f"M3 case: {case}", flush=True)
    try:
        quit_helper = case / "normal-system-quit"
        quit_source = HERE / "macos_system_quit.m"
        build = subprocess.run(["/usr/bin/clang", "-fobjc-arc", "-Wall", "-Wextra", "-Werror",
                                str(quit_source), "-framework", "AppKit", "-lproc", "-o", str(quit_helper)],
                               capture_output=True, text=True, timeout=30, check=False)
        (case / "system-quit-build.log").write_text(build.stdout + build.stderr)
        require(build.returncode == 0, "Native OS quit helper did not compile")
        proof["systemQuitHelper"] = {"sourceSHA256": digest(quit_source), "binarySHA256": digest(quit_helper)}
        for phase in ("", "restart", "reset"):
            name = phase or "first"
            env = dict(os.environ)
            for key in tuple(env):
                if key.startswith("TAKOSUMI_BROWSER_PROCESS_"):
                    del env[key]
            env.update(TAKOSUMI_BROWSER_PROCESS_CASE=str(case), TAKOSUMI_BROWSER_PROCESS_APP=args.site,
                       TAKOSUMI_BROWSER_PROCESS_REAL_SITE="1", TAKOSUMI_BROWSER_PROCESS_PROGRAM=str(browser),
                       TAKOSUMI_BROWSER_PROCESS_GUARD=str(guard), TAKOSUMI_BROWSER_PROCESS_RUNTIME=str(runtime),
                       TAKOSUMI_BROWSER_PROCESS_INSPECT="1",
                       TAKOSUMI_BROWSER_PROCESS_EXTERNAL_LOG=str(case / "external-open.log"))
            if phase:
                env["TAKOSUMI_BROWSER_PROCESS_PHASE"] = phase
            entry = {"name": name}
            proof["phases"].append(entry)
            outer_receipt = case / f"app-guard-{name}.json"
            observer = MacObserver(case, args.site, phase, processes, browser_image, guard_image, quit_helper)
            with open(case / f"app-{name}.log", "xb") as log:
                app_guard = subprocess.Popen([str(guard), str(outer_receipt), str(binary)], env=env,
                                             stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                                             start_new_session=True)
                try:
                    store = observer.marker(f"app-home-store-{name}.json")
                    requested_store = json.loads((case / "home-store.json").read_text())
                    require(store["store"] == requested_store, "App selected another home store")
                    app = processes.facts(store["appPid"])
                    outer = processes.facts(app_guard.pid)
                    require(app and outer and app["parent"] == outer["pid"] and app["start"] >= outer["start"]
                            and app_image.matches(processes.image(app["pid"]))
                            and guard_image.matches(processes.image(outer["pid"]))
                            and processes.facts(app["pid"]) == app and processes.facts(outer["pid"]) == outer,
                            "App is not the owned outer guard's child")
                    observer.app_pid = app["pid"]
                    observer.identities.append(app)
                    entry["homeStore"] = store
                    observer.run()
                    app_guard.wait(timeout=45)
                    require(app_guard.returncode == 0, f"The App guard exited {app_guard.returncode}")
                    entry["appReceipt"] = json.loads(outer_receipt.read_text())
                    normal_receipt(entry["appReceipt"])
                    opened = observer.proof["opened"]
                    entry["profileReceipts"] = [json.loads(Path(item["guardReceipt"]).read_text())
                                                for item in opened["processes"]]
                    for receipt in entry["profileReceipts"]:
                        normal_receipt(receipt)
                    for before in observer.identities:
                        now = processes.facts(before["pid"])
                        require(now is None or now["start"] != before["start"], "An owned browser/guard remains")
                    suffix = f"-{phase}" if phase else ""
                    require(json.loads((case / f"app-browser-process{suffix}-complete.json").read_text())["passed"] is True,
                            "The actual App flow did not complete")
                    entry["cases"] = observer.proof["cases"]
                    entry["identities"] = observer.identities
                    for checked in entry["cases"]:
                        print(f"PASS M3 {args.site} {checked}", flush=True)
                finally:
                    if app_guard.poll() is None:
                        # Popen still owns this unreaped PID; stop only this guard.
                        app_guard.send_signal(signal.SIGTERM)
                        app_guard.wait(timeout=15)
                    if outer_receipt.is_file():
                        entry["outerReceiptText"] = outer_receipt.read_text()
                    try:
                        entry["logs"] = preserve_browser_logs(case, name)
                    except OSError as error:
                        entry["logCopyError"] = str(error)
            entry["passed"] = True
        external = case / "external-open.log"
        require(not external.exists() or external.stat().st_size == 0, "Unexpected external opener request")
        proof["passed"] = True
        print(f"PASS M3 {args.site} all three App launches, normal owned cleanup, no opener request", flush=True)
    except Exception as error:
        proof["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        (case / "summary.json").write_text(json.dumps(proof, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--browser", required=True, type=Path)
    parser.add_argument("--guard", required=True, type=Path)
    parser.add_argument("--work", required=True, type=Path)
    parser.add_argument("--site", required=True, choices=sorted(REAL_SITES))
    args = parser.parse_args()
    require(sys.platform == "darwin", "The M3 native runner requires macOS")
    require(int(subprocess.check_output(["/usr/bin/sw_vers", "-productVersion"], text=True).split('.')[0]) >= 14,
            "The M3 isolated home store requires macOS 14+")
    run(args)


if __name__ == "__main__":
    main()
