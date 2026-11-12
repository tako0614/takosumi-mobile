#!/usr/bin/env python3
"""Actual installed Tauri App Web-home UI on one fresh, owned Simulator."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import stat
import uuid

from harness import runtime
from web_site import Site

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
BUILD = ROOT / "src-tauri/gen/apple/build"
BUNDLE = "com.takosumi.fixture.simulator"
METHOD = "testWholeAppConnectionsWebHomeAndResumeStayProfileBound"
SOURCES = runtime.FROZEN_SOURCES + [HERE / name for name in (
    "run.py", "test_run.py", "harness.py", "web_site.py", "Driver.swift", "WebHomeUITests.swift", "project.yml")]
SOURCES += [ROOT / name for name in (
    "src/entry.tsx", "src/web-home.tsx", "src/web-home.css", "src/web-connections.ts",
    "src/web-profile-recovery.ts", "src/web-profile-data.ts", "src/web-permissions.ts", "index.html",
    "src-tauri/Cargo.toml", "src-tauri/Cargo.lock", "src-tauri/tauri.conf.json",
    "src-tauri/Info.ios.plist", "native-tests/ios/build-simulator.sh", "bun.lock")]


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def digest(path):
    with path.open("rb") as source:
        value = hashlib.sha256()
        for block in iter(lambda: source.read(1024 * 1024), b""):
            value.update(block)
        return value.hexdigest()


def bundle_proof(path, owned_root):
    """Read only an expected Simulator App; bytes stay outside evidence."""
    require(not path.is_symlink(), "Linked App bundle refused")
    root, path = owned_root.resolve(strict=True), path.resolve(strict=True)
    require(path.is_relative_to(root) and path.suffix == ".app" and path.is_dir(),
            "App is outside the exact owning build/container root")
    info_file = path / "Info.plist"
    require(info_file.is_file() and not info_file.is_symlink(), "Plain App Info.plist required")
    with info_file.open("rb") as source:
        info = plistlib.load(source)
    require(info.get("CFBundleIdentifier") == BUNDLE and
            info.get("CFBundleSupportedPlatforms") == ["iPhoneSimulator"], "Wrong App identity/platform")
    executable = info.get("CFBundleExecutable")
    require(isinstance(executable, str) and executable and Path(executable).name == executable,
            "Invalid App executable name")
    entries = {}
    total = 0
    for item in path.rglob("*"):
        before = item.lstat()
        require(not stat.S_ISLNK(before.st_mode), "Linked App content refused")
        if stat.S_ISDIR(before.st_mode):
            continue
        require(stat.S_ISREG(before.st_mode), "Plain App files required")
        total += before.st_size
        require(len(entries) < 10_000 and total <= 2 * 1024**3, "App fingerprint bound exceeded")
        entries[str(item.relative_to(path))] = digest(item)
        after = item.lstat()
        require((before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) ==
                (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns), "App changed during fingerprint")
    require(executable in entries and (path / executable).stat().st_size > 0, "Missing App executable")
    tree = hashlib.sha256(json.dumps(entries, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    return {"bundleIdentifier": BUNDLE, "platform": "iPhoneSimulator", "executable": executable,
            "executableSHA256": entries[executable], "files": len(entries), "bytes": total, "treeSHA256": tree}


def product_app():
    require(BUILD.is_dir(), "Run the existing whole App Simulator SDK build first")
    apps = list(BUILD.rglob("*.app"))
    require(len(apps) == 1, "Exactly one App from the existing Simulator SDK build is required")
    return apps[0], bundle_proof(apps[0], BUILD)


def installed_app(out, udid, device, expected, suffix):
    container = Path(runtime.command(["xcrun", "simctl", "get_app_container", udid, BUNDLE, "app"],
                                     out / f"installed-{suffix}.log", timeout=20).strip())
    data = Path(device.get("dataPath", ""))
    require(data.is_absolute() and data.name == "data" and data.parent.name == udid,
            "Owned Simulator data root unavailable")
    actual = bundle_proof(container, data)
    require(actual == expected, "Installed App differs from the attested SDK build")
    return actual


def require_ui_pass(log):
    identity = f"-[WebHomeUITests.WebHomeUITests {METHOD}]"
    case = re.escape(identity)
    require(re.findall(r"Test Case '([^']+)' started\.", log) == [identity],
            "Unexpected or repeated XCTest case")
    require(len(re.findall(r"Test Case '" + case + r"' started\.", log)) == 1,
            "The exact whole-App test must start once")
    passed = re.findall(r"Test Case '" + case + r"' passed \(([0-9.]+) seconds\)\.", log)
    require(len(passed) == 1 and not re.search(r"Test Case .* (?:failed|skipped)\b", log) and
            re.search(r"Executed 1 test, with 0 failures \(0 unexpected\)", log),
            "Missing nonempty whole-App XCTest success; no skip/restart substitution")
    return {"method": METHOD, "tests": 1, "failures": 0, "seconds": float(passed[0])}


def require_site_events(instance, editor):
    for user in ("Work", "Personal"):
        require(any(e["path"] == "/sign-in/" + user and e["user"] is None for e in instance),
                "Missing fresh profile sign-in request")
        require(any(e["path"] == "/" and e["user"] == user for e in instance),
                "Missing server-authenticated profile home")
    require(sum(e["path"] == "/" and e["user"] is None for e in instance) >= 3 and
            sum(e["path"] == "/sign-in/Work" and e["user"] is None for e in instance) >= 2,
            "Both profiles and the cleared Work must start without an inherited session")
    require(any(e["path"] == "/account" and e["user"] == "Work" for e in instance),
            "The actual Web management page was not fetched")
    require(sum(e["path"] == "/editor" for e in editor) >= 4 and
            any(e["path"] == "/editor/page-two" for e in editor), "Missing cross-origin app navigation/reopening")
    require(all(e["cookiePresent"] is False and e["user"] is None for e in editor),
            "The instance's host cookie reached the distinct editor host")


def retire_device(out, udid, name, selected_runtime):
    devices = json.loads(runtime.command(["xcrun", "simctl", "list", "devices", "--json"],
                                         out / "before-delete.log", timeout=20))
    device = runtime.owned_device(devices, udid, name, selected_runtime)
    if device.get("state") != "Shutdown":
        runtime.command(["xcrun", "simctl", "shutdown", udid], out / "shutdown.log", timeout=60)
    runtime.command(["xcrun", "simctl", "delete", udid], out / "delete.log", timeout=30)
    after = json.loads(runtime.command(["xcrun", "simctl", "list", "devices", "--json"],
                                      out / "after-delete.log", timeout=20))
    require(not any(d.get("udid") == udid for ds in after.get("devices", {}).values() for d in ds),
            "The owned Simulator remains after deletion")


def source_hashes():
    return {str(path.relative_to(ROOT)): digest(path) for path in SOURCES}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    require(platform.system() == "Darwin" and not any(name in os.environ for name in runtime.SIGNING),
            "Requires macOS without signing inputs")
    for tool in ("xcrun", "xcodebuild", "xcodegen", "openssl"):
        require(shutil.which(tool), "Missing existing native tool: " + tool)
    app, app_proof = product_app()
    out = args.out.resolve()
    require(not out.is_relative_to(ROOT), "Evidence must stay outside the repository")
    out.mkdir(mode=0o700)
    scratch_parent = Path(os.environ.get("RUNNER_TEMP", os.environ.get("TMPDIR", "/root/dev/tmp")))
    require(not scratch_parent.resolve().is_relative_to(ROOT), "Private scratch must stay outside the repository")
    require(shutil.disk_usage(scratch_parent).free >= 8 * 1024**3, "Need 8 GiB free; no broad cleanup")
    result = {"passed": False, "scope": "Whole installed Tauri App, synthetic Web UI only",
              "source": source_hashes(), "app": app_proof, "cleanup": False, "functionalPassed": False}
    udid, created, servers, failure = None, None, [], None
    with runtime.owned_scratch(scratch_parent) as (scratch, scratch_state):
        try:
            checkout = runtime.command(["git", "-C", str(ROOT), "rev-parse", "HEAD", "HEAD^{tree}"],
                                       out / "checkout.log").splitlines()
            require(len(checkout) == 2 and all(re.fullmatch(r"[0-9a-f]{40}", v) for v in checkout), "Invalid checkout")
            result["checkout"] = {"head": checkout[0], "tree": checkout[1]}
            kit = ROOT.parent / "mobile-kit"
            dependency = runtime.command(["git", "-C", str(kit), "rev-parse", "HEAD", "HEAD^{tree}"],
                                         out / "kit-checkout.log").splitlines()
            require(len(dependency) == 2 and all(re.fullmatch(r"[0-9a-f]{40}", v) for v in dependency), "Invalid Kit checkout")
            result["kit"] = {"head": dependency[0], "tree": dependency[1]}
            inventory = json.loads(runtime.command(["xcrun", "simctl", "list", "--json"], out / "inventory.log", timeout=20))
            selected_runtime, device_type = runtime.select_runtime(inventory)
            name = "sumi-web-home-" + str(uuid.uuid4())
            udid = runtime.command(["xcrun", "simctl", "create", name, device_type, selected_runtime], out / "create.log").strip()
            require(str(uuid.UUID(udid)).upper() == udid.upper(), "Invalid created Simulator identity")
            devices = json.loads(runtime.command(["xcrun", "simctl", "list", "devices", "--json"], out / "created.log", timeout=20))
            created = runtime.owned_device(devices, udid, name, selected_runtime)
            result["simulator"] = {"udid": udid, "name": name, "runtime": selected_runtime, "deviceType": device_type}
            runtime.save(out / "result.json", result)
            ca, cert, key = runtime.certificate(scratch, localhost=True)
            runtime.command(["xcrun", "simctl", "boot", udid], out / "boot.log", timeout=30)
            runtime.command(["xcrun", "simctl", "bootstatus", udid, "-b"], out / "bootstatus.log", timeout=180, boot_status=True)
            runtime.command(["xcrun", "simctl", "keychain", udid, "add-root-cert", str(ca)], out / "trust.log", trust_trace=True)
            runtime.command(["xcrun", "simctl", "install", udid, str(app)], out / "install.log", timeout=30)
            result["installedBefore"] = installed_app(out, udid, created, app_proof, "before")
            for kind in ("instance", "editor"):
                server = Site(cert, key, kind)
                servers.append(server)
                server.start()
            instance, editor = servers
            instance.editor = editor
            result["origins"] = {"instance": instance.url, "editor": editor.url}
            env = dict(os.environ, SUMI_WEB_HOME_SOURCE=str(HERE), SUMI_WEB_HOME_URL=instance.url,
                       SUMI_WEB_EDITOR_URL=editor.url)
            runtime.command(["xcodegen", "generate", "--spec", str(HERE / "project.yml"),
                             "--project-root", str(scratch), "--project", str(scratch)],
                            out / "project.log", env=env, timeout=30)
            shutil.copyfile(scratch / "WebHomeDriver.xcodeproj/project.pbxproj", out / "project.pbxproj")
            log = runtime.command(["xcodebuild", "test", "-project", str(scratch / "WebHomeDriver.xcodeproj"),
                                   "-scheme", "WebHomeDriver", "-destination", f"id={udid}",
                                   "-derivedDataPath", str(scratch / "derived"), "-resultBundlePath", str(out / "Tests.xcresult"),
                                   "-parallel-testing-enabled", "NO", "-maximum-concurrent-test-simulator-destinations", "1",
                                   "-only-testing:WebHomeUITests/WebHomeUITests/" + METHOD, "CODE_SIGNING_ALLOWED=NO"],
                                  out / "xcodebuild.log", timeout=600, env=env)
            result["xctest"] = require_ui_pass(log)
            result["installedAfter"] = installed_app(out, udid, created, app_proof, "after")
            require(bundle_proof(app, BUILD) == app_proof and source_hashes() == result["source"],
                    "App or source changed during native execution")
            require_site_events(instance.events, editor.events)
            result["functionalPassed"] = True
        except Exception as error:
            failure = error
            result.update(error=str(error), errorType=type(error).__name__, errorNotes=getattr(error, "__notes__", []))
            if created is not None:
                result["deviceLogs"] = runtime.preserve_device_logs(
                    created, udid, out / "owned-device-logs", Path.home() / "Library/Logs/CoreSimulator")
        finally:
            result["testAttachments"] = runtime.export_test_attachments(out)
            for server in servers:
                try:
                    server.stop()
                    runtime.save(out / (server.kind + "-events.json"), server.events)
                except Exception as error:
                    failure = failure or error
                    result["serverCleanupError"] = type(error).__name__
            if created is not None:
                try:
                    retire_device(out, udid, name, selected_runtime)
                    result["cleanup"] = True
                except Exception as error:
                    failure = failure or error
                    result["cleanupError"] = type(error).__name__
            live = [{"pid": item["process"].pid, "log": item["log"]} for item in runtime.UNREAPED_COMMANDS
                    if item["process"].poll() is None]
            if live:
                scratch_state["retain"] = True
                result["activeOwnedCommands"] = live
                failure = failure or RuntimeError("An owned command remains live")
            runtime.save(out / "result.json", result)
    result["scratchCleanup"] = scratch_state
    result["interruptedCommands"] = runtime.INTERRUPTED_COMMANDS
    result["passed"] = (result["functionalPassed"] and result["cleanup"] and failure is None and
                        scratch_state.get("removed") is True and not runtime.INTERRUPTED_COMMANDS)
    if scratch_state["retain"]:
        result["scratchRetained"] = str(scratch)
    runtime.save(out / "result.json", result)
    require(result["passed"], "Whole App iOS Web-home fixture failed; inspect its original result.json")
    print("PASS: whole installed Tauri App connection, synthetic Web login/home, app, Back, profile switch, resume and reset")
    print("Disposable Simulator only; real IdP, physical devices, files/notifications and distribution remain separate")


if __name__ == "__main__":
    main()
