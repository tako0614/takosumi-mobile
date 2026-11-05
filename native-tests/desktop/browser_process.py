#!/usr/bin/env python3
"""Disposable actual Tauri App -> per-profile CEF browser integration probe."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import signal
import subprocess
import sys
import time

from web_home import enable_child_subreaper, wait_engine_children

REPO = Path(__file__).resolve().parents[2]
GUARD = REPO / "native-tests/browser/guard.py"


def normal(receipt):
    assert receipt["rootExit"] == 0 and receipt["complete"] is True
    assert receipt["kernelNoChildren"] is True and receipt["remaining"] == []
    assert receipt["forcedCleanup"] == 0 and receipt["cleanupErrors"] == []
    assert receipt["error"] is None


def digest(file):
    return hashlib.sha256(file.read_bytes()).hexdigest()


def worker(args):
    enable_child_subreaper()
    result = {"passed": False}
    with (args.data / "app.log").open("x") as log:
        app = subprocess.Popen([str(args.binary)], stdout=log, stderr=subprocess.STDOUT)
        try:
            with (args.data / "observer.log").open("x") as observer:
                subprocess.run(["node", str(REPO / "native-tests/desktop/browser-process.mjs"),
                                str(args.data)], check=True, timeout=85,
                               stdout=observer, stderr=subprocess.STDOUT)
            result["appExit"] = app.wait(timeout=15)
            assert result["appExit"] == 0
            assert json.loads((args.data / "app-browser-process-complete.json").read_text())["passed"]
            assert json.loads((args.data / "browser-observation.json").read_text())["passed"]
            opened = json.loads((args.data / "app-browser-process-open.json").read_text())
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
            if app.poll() is None:
                # This exact Popen is owned; its guards receive parent death
                # and drain their descendants. Never scan by executable name.
                app.terminate()
                try:
                    app.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    app.kill()
                    app.wait(timeout=5)
            # Keep this worker's D-Bus/Xvfb session alive while the exact
            # browser guards process parent death and reap their children.
            # Session teardown must not create a D-Bus abort in a live child.
            wait_engine_children({}, {})
            (args.data / "app-execution.json").write_text(json.dumps(result, indent=2)+"\n")


def main():
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--browser", type=Path, required=True)
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--app", choices=["excalidraw", "photopea"], default="excalidraw")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.browser = args.browser.resolve(strict=True)
    args.data = args.data.absolute()
    if args.worker:
        assert args.data.joinpath(".owned-browser-reference").is_file()
        return worker(args)
    if args.data.exists() or not str(args.data).startswith("/root/dev/tmp/"):
        parser.error("--data must be a new owned short directory under /root/dev/tmp")
    args.data.mkdir(mode=0o700)
    args.data.joinpath(".owned-browser-reference").write_text(args.data.name)
    env = dict(os.environ)
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
    files = [args.binary, args.browser, args.browser.parent / "libcef.so", Path(__file__).resolve(),
             REPO / "native-tests/desktop/browser-process.mjs", GUARD]
    changed = subprocess.check_output(["git", "diff", "--name-only", "HEAD"], cwd=REPO, text=True).splitlines()
    added = subprocess.check_output(["git", "ls-files", "--others", "--exclude-standard"], cwd=REPO, text=True).splitlines()
    files = sorted(set(files + [REPO / file for file in changed+added if (REPO / file).is_file()]))
    before = {str(file):digest(file) for file in files}
    receipt = args.data / "session-guard.json"
    command = ["python3", "-B", str(GUARD), str(receipt), "xvfb-run", "-a", "-s", "-screen 0 1280x960x24",
               "dbus-run-session", "--", "python3", "-B", str(Path(__file__).resolve()), "--worker",
               "--binary", str(args.binary), "--browser", str(args.browser), "--data", str(args.data)]
    proof = {"passed":False,"artifactsBefore":before}
    with (args.data / "session.log").open("x") as log:
        process = subprocess.Popen(command, env=env, start_new_session=True, stdout=log, stderr=subprocess.STDOUT)
        try:
            proof["sessionExit"] = process.wait(timeout=110)
            proof["sessionGuard"] = json.loads(receipt.read_text())
            normal(proof["sessionGuard"])
            assert proof["sessionExit"] == 0
            assert json.loads((args.data / "app-execution.json").read_text())["passed"]
            proof["passed"] = True
        finally:
            if process.poll() is None:
                process.send_signal(signal.SIGTERM)
                process.wait(timeout=15)
            proof["artifactsAfter"] = {str(file):digest(file) for file in files}
            if proof["artifactsAfter"] != before:
                proof["passed"] = False
            (args.data / "execution.json").write_text(json.dumps(proof, indent=2)+"\n")
    assert proof["passed"]


if __name__ == "__main__":
    main()
