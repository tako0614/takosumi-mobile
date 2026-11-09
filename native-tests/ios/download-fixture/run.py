#!/usr/bin/env python3
"""Actual product WKDownload UI test on one fresh, owned iOS Simulator."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
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


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def command(argv, path, timeout=60, env=None):
    """Only signal the group created by this still-owned, unreaped Popen."""
    with path.open("xb") as output:
        process = subprocess.Popen(argv, stdout=output, stderr=subprocess.STDOUT,
                                   env=env, start_new_session=True)
        try:
            code = process.wait(timeout=timeout)
        except BaseException:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)
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
    with tempfile.TemporaryDirectory(prefix="sumi-ios-download-", dir=scratch_parent) as raw:
        scratch = Path(raw)
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
            command(["xcrun", "simctl", "bootstatus", udid, "-b"], out / "bootstatus.log", timeout=180)
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
            result["passed"] = result["passed"] and failure is None and result["cleanup"]
            save(out / "result.json", result)
    if failure or not result["passed"]:
        raise RuntimeError(f"iOS download fixture failed; inspect {out / 'result.json'}") from failure
    print("PASS: actual product WKDownload cookie/Blob export, profile isolation and partial-file cancellation")
    print("Disposable Simulator only; no real IdP, physical device, background resume or distribution qualification")


if __name__ == "__main__":
    main()
