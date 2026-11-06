#!/usr/bin/env python3
"""Disposable actual Tauri App -> per-profile CEF browser integration probe."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import signal
import shutil
import subprocess
import sys
import time

import uuid

from browser_apps import create_input
from web_home import enable_child_subreaper, process_identity, wait_engine_children

REPO = Path(__file__).resolve().parents[2]
GUARD = REPO / "native-tests/browser/guard.py"
SCRATCH_ROOT = Path("/root/dev/tmp")
LINUX_SOURCES = (
    "bun.lock", "package.json", "index.html", "vite.config.ts", "tsconfig.json",
    "native-tests/browser/guard.py", "native-tests/desktop/web_home.py",
    "native-tests/desktop/browser_process.py", "native-tests/desktop/browser-process.mjs",
    "native-tests/desktop/browser_snapshot.py", "native-tests/desktop/test_browser_snapshot.py",
    "native-tests/cef-candidate/Cargo.toml", "native-tests/cef-candidate/Cargo.lock",
    "native-tests/cef-candidate/src/process_main.rs", "native-tests/cef-candidate/x11-owner.py",
    "native-tests/cef-candidate/permissions.mjs", "native-tests/cef-candidate/notifications.mjs",
    "native-tests/cef-candidate/notification-backend.mjs", "native-tests/cef-candidate/gtk-readiness.mjs",
    "native-tests/desktop/native-files.mjs", "native-tests/desktop/browser_apps.py",
    "src-tauri/Cargo.toml", "src-tauri/Cargo.lock", "src-tauri/build.rs",
    "src-tauri/src/lib.rs", "src-tauri/src/main.rs", "src-tauri/tests/desktop/browser_process.rs",
    "src-tauri/tauri.conf.json", "src-tauri/tauri.web-client.conf.json",
    "src-tauri/plugins/app-screen/Cargo.toml", "src-tauri/plugins/app-screen/build.rs",
)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def new_case_path(path):
    root = SCRATCH_ROOT.resolve(strict=True)
    parent = path.parent.resolve(strict=True)
    if path.name in {"", ".", ".."} or not parent.is_relative_to(root):
        raise ValueError("Case parent must resolve inside the owned scratch root")
    resolved = parent / path.name
    if resolved.exists() or resolved.is_symlink():
        raise ValueError("Case directory must be new")
    return resolved


def owned_case_path(path):
    root = SCRATCH_ROOT.resolve(strict=True)
    resolved = path.resolve(strict=True)
    if resolved == root or not resolved.is_relative_to(root) or not resolved.is_dir() or not (resolved / ".owned-browser-reference").is_file():
        raise ValueError("Existing case must be owned and inside the scratch root")
    return resolved


def source_files(repo):
    tracked = subprocess.check_output(["git", "ls-files", "--", "src", "src-tauri/plugins/app-screen/src",
                                       "src-tauri/capabilities"], cwd=repo, text=True).splitlines()
    selected = set(LINUX_SOURCES)
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
                f"Expected a local Linux fixture source: {name}")
        result.append(file)
    return result


def normal(receipt):
    require(type(receipt["rootExit"]) is int and receipt["rootExit"] == 0 and receipt["complete"] is True,
            "The guard root must exit normally and completely")
    require(receipt["kernelNoChildren"] is True and receipt["remaining"] == [], "The guard must have no remaining child")
    require(type(receipt["forcedCleanup"]) is int and receipt["forcedCleanup"] == 0 and receipt["cleanupErrors"] == [],
            "The guard must not require forced cleanup")
    require(receipt["error"] is None, "The guard must not report an error")


def digest(file):
    checksum = hashlib.sha256()
    with file.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            checksum.update(chunk)
    return checksum.hexdigest()


def worker(args):
    enable_child_subreaper()
    result = {"passed": False}
    phase = os.environ.get("TAKOSUMI_BROWSER_PROCESS_PHASE", "")
    require(phase in {"", "restart", "reset", "notify", "files"}, "Unknown browser fixture phase")
    suffix = f"-{phase}" if phase else ""
    with (args.data / f"app{suffix}.log").open("x") as log:
        app = subprocess.Popen([str(args.binary)], stdout=log, stderr=subprocess.STDOUT)
        try:
            with (args.data / f"observer{suffix}.log").open("x") as observer:
                subprocess.run(["node", str(REPO / "native-tests/desktop/browser-process.mjs"),
                                str(args.data)], check=True, timeout=85,
                               stdout=observer, stderr=subprocess.STDOUT)
            result["appExit"] = app.wait(timeout=15)
            require(result["appExit"] == 0, "The App must exit normally")
            require(json.loads((args.data / f"app-browser-process{suffix}-complete.json").read_text())["passed"] is True,
                    "The actual App flow must pass")
            require(json.loads((args.data / f"browser-observation{suffix}.json").read_text())["passed"] is True,
                    "The browser observation must pass")
            opened = json.loads((args.data / f"app-browser-process{suffix}-open.json").read_text())
            result["browserGuards"] = []
            for process in opened["processes"]:
                receipt = json.loads(Path(process["guardReceipt"]).read_text())
                normal(receipt)
                result["browserGuards"].append(receipt)
            wait_engine_children({}, {})
            result["passed"] = True
        except Exception as error:
            result["error"] = str(error)
            raise
        finally:
            try:
                if app.poll() is None:
                    # This exact Popen is owned; its guards receive parent death
                    # and drain descendants. Never scan by executable name.
                    app.terminate()
                    try:
                        app.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        app.kill()
                        app.wait(timeout=5)
                # Keep this worker's D-Bus/Xvfb session alive while exact guards
                # process parent death. Don't cause D-Bus aborts in live children.
                wait_engine_children({}, {})
            except Exception as error:
                result["passed"] = False
                result["cleanupError"] = str(error)
                raise
            finally:
                (args.data / f"app-execution{suffix}.json").write_text(json.dumps(result, indent=2)+"\n")


def run_session(args, env, files, before, head, suffix):
    receipt = args.data / f"session-guard{suffix}.json"
    command = ["python3", "-B", str(GUARD), str(receipt), "xvfb-run", "-a", "-s", "-screen 0 1280x960x24",
               "dbus-run-session", "--", "python3", "-B", str(Path(__file__).resolve()), "--worker",
               "--binary", str(args.binary), "--browser", str(args.browser), "--data", str(args.data)]
    proof = {"passed":False,"sourceHead":head,"artifactsBefore":before}
    with (args.data / f"session{suffix}.log").open("x") as log:
        process = subprocess.Popen(command, env=env, start_new_session=True, stdout=log, stderr=subprocess.STDOUT)
        proof["sessionPid"] = process.pid
        identity = process_identity(process.pid)
        proof["sessionStart"] = identity[0] if identity else None
        try:
            proof["sessionExit"] = process.wait(timeout=110)
            proof["sessionGuard"] = json.loads(receipt.read_text())
            normal(proof["sessionGuard"])
            require(proof["sessionExit"] == 0, "The owned session must exit normally")
            require(json.loads((args.data / f"app-execution{suffix}.json").read_text())["passed"] is True,
                    "The actual App execution must pass")
            proof["passed"] = True
        except Exception as error:
            proof["error"] = str(error)
            raise
        finally:
            try:
                if process.poll() is None:
                    process.send_signal(signal.SIGTERM)
                    process.wait(timeout=15)
            except Exception as error:
                proof["passed"] = False
                proof["cleanupError"] = str(error)
                raise
            finally:
                try:
                    proof["artifactsAfter"] = {str(file):digest(file) for file in files}
                    proof["sourceHeadAfter"] = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip()
                    if proof["artifactsAfter"] != before or proof["sourceHeadAfter"] != head:
                        proof["passed"] = False
                except Exception as error:
                    proof["passed"] = False
                    proof["artifactError"] = str(error)
                finally:
                    (args.data / f"execution{suffix}.json").write_text(json.dumps(proof, indent=2)+"\n")
    return proof


def main():
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--browser", type=Path, required=True)
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--app", choices=["excalidraw", "photopea"], default="excalidraw")
    parser.add_argument("--restart", action="store_true",
                        help="relaunch the App on the same data and verify per-profile state after restart")
    parser.add_argument("--reset", action="store_true",
                        help="relaunch, clear Work's site data from the home, and verify only Work was cleared")
    parser.add_argument("--files", action="store_true",
                        help="relaunch and verify the native OS Open/Save As chooser in Work (Excalidraw only)")
    parser.add_argument("--notifications", type=Path,
                        help="relaunch with this owned Dunst binary and verify native permission UI and OS notifications")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.browser = args.browser.resolve(strict=True)
    args.data = args.data.absolute()
    if args.notifications:
        args.notifications = args.notifications.resolve(strict=True)
    if args.files and args.app != "excalidraw":
        parser.error("--files is inspected for --app excalidraw only")
    if args.worker:
        args.data = owned_case_path(args.data)
        return worker(args)
    try:
        args.data = new_case_path(args.data)
    except (OSError, ValueError):
        parser.error("--data must be a new owned short directory under /root/dev/tmp")
    args.data.mkdir(mode=0o700)
    args.data.joinpath(".owned-browser-reference").write_text(args.data.name)
    env = dict(os.environ)
    # Only --restart selects the restart phase; never inherit it from the caller.
    env.pop("TAKOSUMI_BROWSER_PROCESS_PHASE", None)
    for key in ("TAKOSUMI_BROWSER_PROCESS_FILES_INPUT", "TAKOSUMI_BROWSER_PROCESS_FILES_NONCE"):
        env.pop(key, None)
    for key, name in {"HOME":"home", "XDG_DATA_HOME":"data", "XDG_CONFIG_HOME":"config",
                      "XDG_CACHE_HOME":"cache", "XDG_RUNTIME_DIR":"runtime", "TMPDIR":"tmp"}.items():
        directory = args.data / name
        directory.mkdir(mode=0o700)
        env[key] = str(directory)
    runtime = args.data / "r"
    runtime.mkdir(mode=0o700)
    env.update(TAKOSUMI_BROWSER_PROCESS_PROGRAM=str(args.browser),
               TAKOSUMI_BROWSER_PROCESS_GUARD=str(GUARD),
               TAKOSUMI_BROWSER_PROCESS_RUNTIME=str(runtime),
               TAKOSUMI_BROWSER_PROCESS_CASE=str(args.data),
               TAKOSUMI_BROWSER_PROCESS_INSPECT="1",
               TAKOSUMI_BROWSER_PROCESS_APP=args.app,
               WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS="1", G_DEBUG="fatal-criticals")
    # A recording stand-in for the desktop's opener, first on PATH: the App hands
    # links the person chose to open outside to xdg-open. Nothing real opens.
    opener = args.data / "opener"
    opener.mkdir(mode=0o700)
    stub = opener / "xdg-open"
    # The log path is built in, not read from the environment: a profile
    # browser (whose environment is cleared) starting xdg-open is recorded too.
    log = args.data / "external-open.log"
    stub.write_text(f"#!{sys.executable} -I\nimport json, os, sys\n"
                    "path = os.environ.get('TAKOSUMI_BROWSER_PROCESS_EXTERNAL_SELFTEST') or " + repr(str(log)) + "\n"
                    "with open(path, 'a') as out:\n"
                    "    out.write(json.dumps({'argv': sys.argv[1:], 'ppid': os.getppid(),"
                    " 'groupLeader': os.getpgrp() == os.getpid()}) + '\\n')\n")
    stub.chmod(0o700)
    env["PATH"] = f"{opener}{os.pathsep}{env.get('PATH', '')}"
    env["TAKOSUMI_BROWSER_PROCESS_EXTERNAL_LOG"] = str(log)
    # The stand-in must be what the App resolves, and it must actually run:
    # a failing exec would make PATH lookup fall through to the real opener.
    require(shutil.which("xdg-open", path=env["PATH"]) == str(stub), "The recording opener is not first on PATH")
    selftest = opener / "selftest.log"
    subprocess.run([str(stub), "selftest"], env=dict(env, TAKOSUMI_BROWSER_PROCESS_EXTERNAL_SELFTEST=str(selftest)),
                   check=True, timeout=30)
    require(json.loads(selftest.read_text())["argv"] == ["selftest"], "The recording opener did not record")
    file_input = file_nonce = None
    if args.files:
        # The owned input is hashed with the sources: Save As must not overwrite it.
        (args.data / "files").mkdir(mode=0o700)
        file_nonce = str(uuid.uuid4())
        file_input = create_input(args.data / "files", "excalidraw", file_nonce)
    files = sorted(set([args.binary, args.browser, args.browser.parent / "libcef.so"] + source_files(REPO)
                       + ([args.notifications] if args.notifications else [])
                       + ([file_input] if file_input else [])))
    before = {str(file):digest(file) for file in files}
    # Preserve small source bytes and the executable inodes used in this case.
    # These links are retention only; processes still use the attested original
    # paths and any in-run byte change makes the result fail.
    snapshot = args.data / "source-snapshot"
    snapshot.mkdir(mode=0o700)
    for file in files:
        if file.is_relative_to(REPO) and file.stat().st_size <= 2 * 1024 * 1024:
            target = snapshot / file.relative_to(REPO)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(file, target)
    os.link(args.binary, args.data / "app-executable")
    os.link(args.browser, args.data / "browser-executable")
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip()
    proof = run_session(args, env, files, before, head, "")
    require(proof["passed"] is True, "The native execution did not pass")
    if args.restart:
        # Relaunch the App on the same HOME/XDG data: each profile's site state
        # must survive a normal exit and stay separated after restart.
        restart = run_session(args, dict(env, TAKOSUMI_BROWSER_PROCESS_PHASE="restart"), files, before, head, "-restart")
        require(restart["passed"] is True, "The restarted native execution did not pass")
    if args.reset:
        # Another launch on the same data: clear Work from the home's own UI;
        # Personal (and a clear attempt while it runs) must keep its state.
        reset = run_session(args, dict(env, TAKOSUMI_BROWSER_PROCESS_PHASE="reset"), files, before, head, "-reset")
        require(reset["passed"] is True, "The reset native execution did not pass")
    if args.notifications:
        # Another launch: native Chrome permission UI and real OS notifications
        # per profile, through an owned notification daemon on the session bus.
        notify = run_session(args, dict(env, TAKOSUMI_BROWSER_PROCESS_PHASE="notify",
                                        TAKOSUMI_BROWSER_PROCESS_NOTIFIER=str(args.notifications)),
                             files, before, head, "-notify")
        require(notify["passed"] is True, "The notification native execution did not pass")
    if args.files:
        files_run = run_session(args, dict(env, TAKOSUMI_BROWSER_PROCESS_PHASE="files",
                                           TAKOSUMI_BROWSER_PROCESS_FILES_INPUT=str(file_input),
                                           TAKOSUMI_BROWSER_PROCESS_FILES_NONCE=file_nonce),
                                files, before, head, "-files")
        require(files_run["passed"] is True, "The native file chooser execution did not pass")
    # Over all phases, only the first launch's chosen link reached the opener.
    opened = (args.data / "external-open.log").read_text().splitlines()
    require(len(opened) == 1, f"Expected exactly one external open, saw {len(opened)}")


if __name__ == "__main__":
    main()
