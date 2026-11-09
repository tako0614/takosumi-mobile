#!/usr/bin/env python3
"""Actual product WKDownload UI test on one fresh, owned iOS Simulator."""
import argparse
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import stat
import subprocess
import tempfile
import time
import uuid

from server import BLOB_BYTES, COOKIE_BYTES, SENTINEL_BYTES, FixtureServer

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
PRODUCT = ROOT / "src-tauri/plugins/app-screen/ios/Sources"
SOURCES = [PRODUCT / name for name in (
    "AppScreenViewController.swift", "AppScreenPolicy.swift", "AppScreenDownload.swift")]
FROZEN_SOURCES = SOURCES + [HERE / name for name in (
    "App.swift", "DownloadUITests.swift", "project.yml", "run.py", "server.py", "test_run.py")]
BUNDLE = "com.takosumi.fixture.download"
SIGNING = ("IOS_CERTIFICATE", "IOS_CERTIFICATE_PASSWORD", "IOS_MOBILE_PROVISION",
           "APPLE_DEVELOPMENT_TEAM", "APPLE_API_KEY", "APPLE_API_ISSUER", "APPLE_API_KEY_PATH")
UNREAPED_COMMANDS = []
INTERRUPTED_COMMANDS = []


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def migration_progress(path):
    """Read native migration progress and explicit terminal success from this CLI."""
    with path.open("rb") as source:
        source.seek(0, 2)
        source.seek(max(0, source.tell() - 262144))
        text = source.read().decode("utf-8", errors="replace")
    transitions, elapsed, status, terminal, finished = [], 0, None, None, False
    for line in text.splitlines():
        header = re.search(r"Status=(\d+), isTerminal=(YES|NO), Elapsed=(\d+):(\d+)\.", line)
        if header:
            status, terminal = int(header[1]), header[2]
            elapsed = int(header[3]) * 60 + int(header[4])
            finished = False
        elif line.strip() == "Finished" and status == 4294967295 and terminal == "YES":
            finished = True
        plugin = re.search(r"Reason:Running plugin ([\w.-]+) \(", line)
        if plugin and status == 2 and terminal == "NO":
            if not transitions or transitions[-1]["plugin"] != plugin[1]:
                transitions.append({"plugin": plugin[1], "elapsed": elapsed})
    recent = (len(transitions) >= 2 and status == 2 and terminal == "NO" and
              elapsed >= 150 and 0 <= elapsed - transitions[-1]["elapsed"] <= 60)
    return {"transitions": transitions, "lastElapsed": elapsed, "recentMigrationProgress": recent,
            "extensionSeconds": 120 if recent else 0, "maximumBootSeconds": 300,
            "terminalReady": finished}


def retire_cli(process, path, original):
    # A simctl process group can contain platform-owned helpers. Own only the
    # direct, unreaped Popen child; Simulator teardown uses its exact UDID.
    outcome = {"pid": process.pid, "log": str(path), "originalErrorType": type(original).__name__,
               "reaped": False, "signals": []}
    INTERRUPTED_COMMANDS.append(outcome)
    code = process.poll()
    if code is not None:
        outcome.update(reaped=True, exit=code)
    else:
        for label, stop in (("terminate", process.terminate), ("kill", process.kill)):
            event = {"operation": label}
            try:
                stop()
            except OSError as error:
                event["errorType"] = type(error).__name__
            outcome["signals"].append(event)
            try:
                outcome.update(reaped=True, exit=process.wait(timeout=5))
                break
            except subprocess.TimeoutExpired:
                event["waitTimedOut"] = True
    if not outcome["reaped"]:
        UNREAPED_COMMANDS.append({"process": process, "log": str(path)})
    try:
        save(path.with_suffix(".process.json"), outcome)
    except OSError as error:
        original.add_note(f"Command retirement receipt failed: {type(error).__name__}")


@contextmanager
def owned_scratch(parent):
    path = Path(tempfile.mkdtemp(prefix="sumi-ios-download-", dir=parent))
    state = {"retain": False, "removed": False}
    try:
        yield path, state
    finally:
        # Reaping xcodebuild (or another CLI) does not prove its descendants
        # stopped using the build directory. Preserve all interrupted scratch.
        if INTERRUPTED_COMMANDS:
            state["retain"] = True
            state["reason"] = "CLI interrupted; descendant scratch use is not disproved"
        if not state["retain"]:
            try:
                shutil.rmtree(path)
                state["removed"] = True
            except OSError as error:
                state["cleanupError"] = type(error).__name__


def command(argv, path, timeout=60, env=None, boot_status=False):
    """Preserve the original failure even if retiring our own CLI also fails."""
    if boot_status and (len(argv) != 5 or argv[:3] != ["xcrun", "simctl", "bootstatus"] or
                        argv[4] != "-b" or timeout != 180):
        raise ValueError("Cold-boot continuation is only for exact Simulator readiness")
    with path.open("xb") as output:
        process = subprocess.Popen(argv, stdout=output, stderr=subprocess.STDOUT,
                                   env=env, start_new_session=True)
        try:
            try:
                code = process.wait(timeout=timeout)
            except subprocess.TimeoutExpired as original:
                if not boot_status:
                    raise
                try:
                    progress = migration_progress(path)
                    progress["udid"] = argv[3]
                    if progress["terminalReady"]:
                        # The child can finish between wait's deadline and
                        # this readback. Observe it; do not wait or signal it.
                        progress["deadlineBoundaryExit"] = process.poll()
                    save(path.with_suffix(".progress.json"), progress)
                except OSError as error:
                    original.add_note(f"No recorded migration progress: {type(error).__name__}")
                    raise original
                if progress["terminalReady"] and progress.get("deadlineBoundaryExit") == 0:
                    code = 0
                elif not progress["recentMigrationProgress"]:
                    raise
                else:
                    # Continue this same readiness process once, without reboot,
                    # only after actual plugin transitions near the first deadline.
                    try:
                        code = process.wait(timeout=120)
                    except subprocess.TimeoutExpired:
                        original.add_note("Fresh-boot migration continued for 120 seconds, but readiness still timed out")
                        raise original
        except BaseException as original:
            try:
                retire_cli(process, path, original)
            except Exception as error:
                original.add_note(f"Owned CLI retirement failed: {type(error).__name__}")
                UNREAPED_COMMANDS.append({"process": process, "log": str(path)})
            raise
    if code:
        raise RuntimeError(f"{argv[0]} exit {code}; see {path.name}")
    return path.read_text(errors="replace")


def certificate(directory):
    """Private key files stay outside uploaded evidence and source."""
    ca, cert, key = directory / "ca.pem", directory / "server.pem", directory / "server.key"
    command(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
             "-subj", "/CN=Disposable download fixture CA", "-addext", "basicConstraints=critical,CA:TRUE",
             "-addext", "keyUsage=critical,keyCertSign,cRLSign",
             "-keyout", str(directory / "ca.key"), "-out", str(ca)], directory / "ca.log")
    command(["openssl", "req", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=127.0.0.1",
             "-keyout", str(key), "-out", str(directory / "server.csr")], directory / "request.log")
    extensions = directory / "server.ext"
    extensions.write_text("subjectAltName=IP:127.0.0.1\nextendedKeyUsage=serverAuth\n"
                          "keyUsage=critical,digitalSignature,keyEncipherment\nbasicConstraints=critical,CA:FALSE\n")
    command(["openssl", "x509", "-req", "-days", "1", "-in", str(directory / "server.csr"),
             "-CA", str(ca), "-CAkey", str(directory / "ca.key"), "-CAcreateserial",
             "-extfile", str(extensions), "-out", str(cert)], directory / "certificate.log")
    for private in (key, directory / "ca.key"):
        private.chmod(0o600)
    return ca, cert, key


def select_runtime(inventory):
    candidates = []
    for runtime in inventory.get("runtimes", []):
        version = tuple(int(n) for n in runtime.get("version", "0").split("."))
        if not (runtime.get("isAvailable") is True and version >= (17,) and
                runtime.get("identifier", "").startswith("com.apple.CoreSimulator.SimRuntime.iOS-")):
            continue
        phones = [d for d in runtime.get("supportedDeviceTypes", [])
                  if d.get("name", "").startswith("iPhone ") and d.get("identifier")]
        if phones:
            candidates.append((version, runtime["identifier"], phones[-1]["identifier"]))
    if not candidates:
        raise RuntimeError("No available iOS 17+ runtime with an explicit supported iPhone; no install or skip")
    return max(candidates)[1:]


def owned_device(inventory, udid, name, runtime):
    found = [(r, d) for r, devices in inventory.get("devices", {}).items()
             for d in devices if d.get("udid") == udid]
    if len(found) != 1 or found[0][0] != runtime or found[0][1].get("name") != name:
        raise RuntimeError("Simulator ownership readback mismatch; no shutdown/delete")
    return found[0][1]


def require_test_pass(log):
    # Require the actual method and aggregate, never only xcodebuild exit zero.
    method = "testOriginalResponsesExportAndRetireWithinTheirProfile"
    if not (re.search(r"Test [Cc]ase [^\n]*DownloadUITests[^\n]*" + method + r"[^\n]* passed", log) and
            re.search(r"Executed 1 test, with 0 failures \(0 unexpected\)", log) and
            "** TEST SUCCEEDED **" in log and "** TEST FAILED **" not in log):
        raise RuntimeError("Missing successful nonempty actual XCTest result")


def read_export(documents, name, limit=16384):
    path = documents / name
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_size > limit:
        raise RuntimeError(f"Unsafe or oversized actual export: {name}")
    return path.read_bytes()


def verify_exports(documents, evidence):
    expected = {"cookie.txt": COOKIE_BYTES, "blob.txt": BLOB_BYTES, "keep.txt": SENTINEL_BYTES}
    digests = {}
    for name, data in expected.items():
        actual = read_export(documents, name)
        if actual != data:
            raise RuntimeError(f"OS export bytes differ: {name}")
        (evidence / name).write_bytes(actual)
        digests[name] = hashlib.sha256(actual).hexdigest()
    receipt = json.loads(read_export(documents, "fixture-receipt.json"))
    if receipt != {"cookie": True, "blob": True, "temporaryCount": 0, "sentinel": True,
                   "closedDuringTransfer": True, "closeTimedOut": False}:
        raise RuntimeError("Incomplete actual App file/teardown observations")
    save(evidence / "fixture-receipt.json", receipt)
    return digests


def require_interrupted_transfers(events):
    ended = [e for e in events if e.get("path") == "/slow.bin" and e.get("ended") is True]
    if len(ended) != 2 or any(e.get("peerClosed") is not True or e.get("completed") is not False or
                              not 0 < e.get("chunks", 0) < 512 for e in ended):
        raise RuntimeError("Need two actual incomplete responses disconnected by WebKit, before server shutdown")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    if platform.system() != "Darwin" or any(name in os.environ for name in SIGNING):
        raise RuntimeError("Requires macOS without Apple signing inputs")
    for tool in ("xcrun", "xcodebuild", "xcodegen", "openssl"):
        if not shutil.which(tool):
            raise RuntimeError(f"Missing existing tool: {tool}")
    out = args.out.absolute()
    out.mkdir(mode=0o700)  # Fresh evidence only; never reuse an old pass.
    scratch_parent = Path(os.environ.get("RUNNER_TEMP", os.environ.get("TMPDIR", "/root/dev/tmp")))
    if shutil.disk_usage(scratch_parent).free < 8 * 1024**3:
        raise RuntimeError("Need 8 GiB free for one Simulator/build; no broad cleanup")
    result = {"passed": False, "simulator": None, "cleanup": False,
              "source": {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in FROZEN_SOURCES}}
    udid = None
    server = None
    failure = None
    with owned_scratch(scratch_parent) as (scratch, scratch_state):
        try:
            checkout = command(["git", "-C", str(ROOT), "rev-parse", "HEAD", "HEAD^{tree}"], out / "checkout.log").splitlines()
            if len(checkout) != 2 or any(not re.fullmatch(r"[0-9a-f]{40}", value) for value in checkout):
                raise RuntimeError("Missing checkout head/tree identity")
            result["checkout"] = {"head": checkout[0], "tree": checkout[1]}
            inventory = json.loads(command(["xcrun", "simctl", "list", "--json"], out / "inventory.log"))
            runtime, device_type = select_runtime(inventory)
            name = "sumi-download-" + str(uuid.uuid4())
            udid = command(["xcrun", "simctl", "create", name, device_type, runtime], out / "create.log").strip()
            if str(uuid.UUID(udid)).upper() != udid.upper():
                raise RuntimeError("Invalid created Simulator identity")
            result["simulator"] = {"udid": udid, "name": name, "runtime": runtime, "deviceType": device_type}
            devices = json.loads(command(["xcrun", "simctl", "list", "devices", "--json"], out / "created.log"))
            owned_device(devices, udid, name, runtime)
            save(out / "result.json", result)
            ca, cert, key = certificate(scratch)
            command(["xcrun", "simctl", "boot", udid], out / "boot.log")
            command(["xcrun", "simctl", "bootstatus", udid, "-b"], out / "bootstatus.log", timeout=180, boot_status=True)
            # Trust only on this exact disposable Simulator, never host trust.
            command(["xcrun", "simctl", "keychain", udid, "add-root-cert", str(ca)], out / "trust.log")
            server = FixtureServer(cert, key)
            server.start()
            env = dict(os.environ, SUMI_DOWNLOAD_URL=server.url, SUMI_DOWNLOAD_SOURCE=str(HERE),
                       SUMI_DOWNLOAD_PRODUCT=str(PRODUCT))
            # Absolute source references keep generated project/build output off source.
            command(["xcodegen", "generate", "--spec", str(HERE / "project.yml"), "--project", str(scratch)],
                    out / "project.log", env=env)
            log = command(["xcodebuild", "test", "-project", str(scratch / "DownloadFixture.xcodeproj"),
                           "-scheme", "DownloadFixture", "-destination", f"id={udid}",
                           "-derivedDataPath", str(scratch / "derived"), "-resultBundlePath", str(out / "Tests.xcresult"),
                           "-parallel-testing-enabled", "NO", "-maximum-concurrent-test-simulator-destinations", "1",
                           "CODE_SIGNING_ALLOWED=NO"], out / "xcodebuild.log", timeout=900, env=env)
            require_test_pass(log)
            container = Path(command(["xcrun", "simctl", "get_app_container", udid, BUNDLE, "data"],
                                     out / "container.log").strip())
            if not container.is_absolute() or not container.is_dir() or container.is_symlink():
                raise RuntimeError("Invalid actual App container")
            result["exports"] = verify_exports(container / "Documents", out)
            deadline = time.monotonic() + 10
            while len([e for e in server.events if e.get("ended")]) < 2 and time.monotonic() < deadline:
                time.sleep(0.1)
            require_interrupted_transfers(server.events)
            if result["source"] != {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in FROZEN_SOURCES}:
                raise RuntimeError("Product/fixture source changed during native test")
            result["passed"] = True
        except Exception as error:
            failure = error
            result["error"] = str(error)
            result["errorType"] = type(error).__name__
            result["errorNotes"] = getattr(error, "__notes__", [])
        finally:
            try:
                if server is not None:
                    server.stop()
                    save(out / "server-events.json", server.events)
            except Exception as error:
                failure = failure or error
                result["serverCleanupError"] = str(error)
            if udid is not None:
                try:
                    devices = json.loads(command(["xcrun", "simctl", "list", "devices", "--json"], out / "before-delete.log"))
                    device = owned_device(devices, udid, name, runtime)
                    if device.get("state") != "Shutdown":
                        command(["xcrun", "simctl", "shutdown", udid], out / "shutdown.log", timeout=60)
                    command(["xcrun", "simctl", "delete", udid], out / "delete.log")
                    after = json.loads(command(["xcrun", "simctl", "list", "devices", "--json"], out / "after-delete.log"))
                    if any(d.get("udid") == udid for ds in after.get("devices", {}).values() for d in ds):
                        raise RuntimeError("Owned Simulator still exists after delete")
                    result["cleanup"] = True
                except Exception as error:
                    failure = failure or error
                    result["cleanupError"] = str(error)
            live = [{"pid": item["process"].pid, "log": item["log"]} for item in UNREAPED_COMMANDS
                    if item["process"].poll() is None]
            if live:
                scratch_state["retain"] = True
                result["scratchRetained"] = str(scratch)
                result["activeOwnedCommands"] = live
                failure = failure or RuntimeError("An owned CLI is still live; preserve its scratch")
            result["passed"] = result["passed"] and failure is None and result["cleanup"]
            save(out / "result.json", result)
    result["scratchCleanup"] = scratch_state
    result["interruptedCommands"] = INTERRUPTED_COMMANDS
    if scratch_state["retain"]:
        result["scratchRetained"] = str(scratch)
    if scratch_state.get("cleanupError"):
        failure = failure or RuntimeError("Owned scratch cleanup failed")
        result["passed"] = False
    save(out / "result.json", result)
    if failure or not result["passed"]:
        raise RuntimeError(f"iOS download fixture failed; inspect {out / 'result.json'}") from failure
    print("PASS: actual product WKDownload cookie/Blob export, profile isolation and partial-file cancellation")
    print("Disposable Simulator only; no real IdP, physical device, background resume or distribution qualification")


if __name__ == "__main__":
    main()
