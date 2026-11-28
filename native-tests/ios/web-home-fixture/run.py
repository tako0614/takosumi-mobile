#!/usr/bin/env python3
"""Actual installed Tauri App Web-home UI on one fresh, owned Simulator."""
import argparse
from datetime import datetime, timezone
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import stat
import struct
import subprocess
import tarfile
import zipfile
import uuid

from contextlib import ExitStack

from harness import runtime
from web_site import Site

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
BUILD = ROOT / "src-tauri/gen/apple/build"
BUNDLE = "com.takosumi.fixture.simulator"
METHOD = "testWholeAppConnectionsWebHomeAndResumeStayProfileBound"
# The first actual 128 MB Tauri App install outlived the initial 30 s
# allowance. One bounded setup attempt; functional XCTest still gets 600 s
# and installation must finish normally before exact bundle readback.
INSTALL_WAIT = 120
NAVIGATION_LOG_LIMIT = 512 * 1024
FILTERED_NAVIGATION_WAIT = 8
SOURCES = runtime.FROZEN_SOURCES + [HERE / name for name in (
    "run.py", "test_run.py", "harness.py", "web_site.py", "Driver.swift", "WebHomeUITests.swift", "project.yml")]
SOURCES += [ROOT / name for name in (
    "src/entry.tsx", "src/web-home.tsx", "src/web-home.css", "src/web-connections.ts",
    "src/web-profile-recovery.ts", "src/web-profile-data.ts", "src/web-permissions.ts", "index.html",
    "src/fixture-bootstrap.ts", "tests/fixture-bootstrap.test.ts",
    "src-tauri/src/lib.rs", "src-tauri/src/bootstrap_fixture.rs",
    "src-tauri/Cargo.toml", "src-tauri/Cargo.lock", "src-tauri/tauri.conf.json",
    "src-tauri/Info.ios.plist", "src-tauri/ios-project.yml", "native-tests/ios/build-simulator.sh",
    "native-tests/ios/swift_runtime.py", "native-tests/ios/test_swift_runtime.py", "bun.lock")]


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


def navigation_witness(data):
    """Closed producer metadata from the already-retained tail, never a navigation verdict."""
    result = {"diagnosticOnly": True, "clockDomain": "ios_system_uptime", "clockUnit": "ms",
              "records": [], "recordLimit": 513, "dropped": 0, "malformed": 0,
              "capStatus": "not_observed", "absenceIsUnknown": True}
    if len(data) > NAVIGATION_LOG_LIMIT:
        result["errorType"] = "InputBoundExceeded"
        return result
    marker = b"com.takosumi.fixture.simulator.nav "
    pattern = re.compile(rb"v=1 seq=([1-9][0-9]{0,2}) event=(open|request|home_request|returned|provisional|commit|finish|"
                         rb"fail_provisional|fail|cap|policy_allow|policy_allow_returned|response_policy_entered|"
                         rb"response_policy_allow_returned|content_terminated) "
                         rb"view=([0-9]{1,5}) request=([0-9]{1,5}) relation=([012]) target=(instance|editor|other|unknown|editor_page_two) "
                         rb"reused=([01]) kept=([01]) resume=([01]) initial=([01]) sameView=([01]) timeMs=([0-9]{1,15})")
    for line in data.splitlines():
        if marker not in line:
            continue
        prefix, payload = line.split(marker, 1)
        # A marker in page/WebKit text is not our native producer. Match the
        # complete saved compact-log header, including its fixed subsystem.
        emitter = re.fullmatch(rb"[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2}\.[0-9]+\s+Df\s+"
                               rb"Takosumi\[([1-9][0-9]{0,9}):[0-9a-fA-Fx]+\]\s+"
                               rb"\[com\.takosumi\.fixture\.simulator\.navigation:navigation\]\s+", prefix)
        match = pattern.fullmatch(payload)
        if not emitter or not match or int(match[1]) > 513 or int(match[3]) > 65_535 or int(match[4]) > 65_535:
            result["malformed"] = min(513, result["malformed"] + 1)
            continue
        record = {"pid": int(emitter[1]), "seq": int(match[1]),
                  "event": match[2].decode("ascii"), "view": int(match[3]), "request": int(match[4]),
                  "relation": int(match[5]), "target": match[6].decode("ascii"),
                  "reused": match[7] == b"1", "kept": match[8] == b"1", "resume": match[9] == b"1",
                  "initial": match[10] == b"1", "sameView": match[11] == b"1", "timeMs": int(match[12])}
        policy = record["event"] in ("policy_allow", "policy_allow_returned",
                                     "response_policy_entered", "response_policy_allow_returned")
        if ((policy and (record["seq"] == 513 or record["view"] == 0 or record["request"] != 0
                         or record["relation"] != 0 or not record["sameView"]
                         or any(record[key] for key in ("reused", "kept", "resume", "initial"))))
                or (not policy and record["target"] == "editor_page_two")):
            result["malformed"] = min(513, result["malformed"] + 1)
            continue  # Policy metadata never supplies request or callback context.
        terminated = record["event"] == "content_terminated"
        if terminated and (record["seq"] == 513 or record["view"] == 0 or record["request"] != 0
                           or record["relation"] != 0 or record["target"] != "unknown" or not record["sameView"]
                           or any(record[key] for key in ("reused", "kept", "resume", "initial"))):
            result["malformed"] = min(513, result["malformed"] + 1)
            continue  # An owner-bound delegate event is never navigation context or a liveness verdict.
        if record["event"] == "home_request" and (
                record["seq"] == 513 or record["view"] == 0 or record["request"] == 0
                or record["relation"] != 0 or record["target"] == "unknown" or not record["sameView"]
                or not record["reused"] or record["kept"] or record["resume"] or not record["initial"]):
            result["malformed"] = min(513, result["malformed"] + 1)
            continue  # The menu requests a launch URL in an already-bound view.
        callback = record["event"] in ("provisional", "commit", "finish", "fail_provisional", "fail")
        matched = record["relation"] == 1 and record["sameView"]
        context = (record["request"] != 0 or record["target"] != "unknown"
                   or any(record[key] for key in ("reused", "kept", "resume", "initial")))
        if callback and not matched and context:
            result["malformed"] = min(513, result["malformed"] + 1)
            continue  # Keep unmatched callback context unknown, including old producer packets.
        if len(result["records"]) == 513:
            # Keep the latest process/view window, including a cold relaunch.
            result["records"].pop(0)
            result["dropped"] = min(513, result["dropped"] + 1)
        result["records"].append(record)
        if record["event"] == "cap":
            result["capStatus"] = "observed"
    return result


def screen_witness(source, size, *, partial):
    """Closed prefix projection from the existing query, never a readiness receipt."""
    result = {"diagnosticOnly": True, "clockDomain": "ios_native_os_log_header", "records": [],
              "recordLimit": 65, "scanLimitBytes": 2 * 1024 * 1024, "scanLineLimit": 8192,
              "lineLimitBytes": 512, "savedByteLimit": 32 * 1024, "sourceBytes": size,
              "partial": partial, "absenceIsUnknown": True, "dropped": 0, "malformed": 0,
              "capStatus": "not_observed", "navigationCapObserved": False}
    data = source.read(min(size, result["scanLimitBytes"]))
    result["readBytes"] = len(data)
    result["scanTruncated"] = size > len(data)
    lines = data.splitlines()
    # A cut packet cannot acquire a binding from an unobserved suffix.
    if (partial or result["scanTruncated"]) and data and not data.endswith((b"\n", b"\r")):
        lines = lines[:-1]
    result["lineTruncated"] = len(lines) > result["scanLineLimit"]
    marker = b"com.takosumi.fixture.simulator.screen "
    nav_marker = b"com.takosumi.fixture.simulator.nav "
    header = re.compile(rb"([0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2}\.[0-9]{1,9})\s+Df\s+"
                        rb"Takosumi\[([1-9][0-9]{0,9}):[0-9a-fA-Fx]+\]\s+"
                        rb"\[com\.takosumi\.fixture\.simulator\.navigation:navigation\]\s+")
    packet = re.compile(rb"v=1 seq=([1-9][0-9]?) stage=(requested|appeared|cap) screen=([0-9]{1,5}) view=([0-9]{1,5})")
    callbacks = {"provisional", "commit", "finish", "fail_provisional", "fail", "content_terminated"}
    for line in lines[:result["scanLineLimit"]]:
        if marker not in line and nav_marker not in line:
            continue
        if len(line) > result["lineLimitBytes"]:
            result["malformed"] = min(65, result["malformed"] + 1)
            continue
        prefix, payload = line.split(marker if marker in line else nav_marker, 1)
        emitter = header.fullmatch(prefix)
        record = None
        if emitter and marker in line:
            match = packet.fullmatch(payload)
            if match:
                seq, stage, screen, view = int(match[1]), match[2].decode("ascii"), int(match[3]), int(match[4])
                valid = ((stage == "cap" and seq == 65 and screen == view == 0) or
                         (stage != "cap" and seq <= 64 and 0 < screen <= 65_535 and 0 < view <= 65_535))
                if valid:
                    record = {"kind": "presentation", "seq": seq, "stage": stage, "screen": screen,
                              "view": view, "sameView": None}
                    if stage == "cap":
                        result["capStatus"] = "observed"
        elif emitter:
            # Reuse the old strict decoder; do not supply its unknown callback context.
            decoded = navigation_witness(line)
            if len(decoded["records"]) == 1 and not decoded["malformed"]:
                nav = decoded["records"][0]
                if nav["event"] == "cap":
                    result["navigationCapObserved"] = True
                    continue
                if nav["event"] not in callbacks:
                    continue
                record = {"kind": "navigation", "seq": nav["seq"], "stage": nav["event"], "screen": 0,
                          "view": nav["view"] if nav["sameView"] else 0, "sameView": nav["sameView"]}
        if record is None:
            result["malformed"] = min(65, result["malformed"] + 1)
            continue
        record.update(pid=int(emitter[2]), osLogTime=emitter[1].decode("ascii"))
        if len(result["records"]) == result["recordLimit"]:
            result["dropped"] = min(65, result["dropped"] + 1)
        else:
            result["records"].append(record)
    # Include embedding indentation in the explicit saved projection bound.
    if len(json.dumps({"targetAppExit": {"screenWitness": result}}, indent=2).encode()) > result["savedByteLimit"]:
        result["records"] = []
        result["errorType"] = "ProjectionBoundExceeded"
    return result


def bootstrap_witness(data, size, *, partial):
    """Closed fixture-event hints from the shared prefix; never a display verdict."""
    result = {"diagnosticOnly": True, "clockDomain": "browser_performance_time_origin_ms",
              "osLogClock": "ios_unified_log_header_delivery_time", "timestampsComparableAcrossDomains": False,
              "sequenceMayRepeatAcrossRealms": True, "absenceIsUnknown": True, "records": [],
              "recordLimit": 33, "scanLimitBytes": 2 * 1024 * 1024, "scanLineLimit": 8192,
              "lineLimitBytes": 512, "savedByteLimit": 16 * 1024, "sourceBytes": size,
              "partial": partial, "dropped": 0, "malformed": 0, "capStatus": "not_observed"}
    if not isinstance(data, bytes) or len(data) > min(size, result["scanLimitBytes"]):
        result["errorType"] = "InputBoundExceeded"
        return result
    result["readBytes"] = len(data)
    result["scanTruncated"] = size > len(data)
    lines = data.splitlines()
    if (partial or result["scanTruncated"]) and data and not data.endswith((b"\n", b"\r")):
        lines = lines[:-1]
    result["lineTruncated"] = len(lines) > result["scanLineLimit"]
    marker = b"com.takosumi.fixture.simulator.bootstrap "
    header = re.compile(rb"([0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2}\.[0-9]{1,9})\s+Df\s+"
                        rb"Takosumi\[([1-9][0-9]{0,9}):[0-9a-fA-Fx]+\]\s+"
                        rb"\[com\.takosumi\.fixture\.simulator:app\]\s+\[stderr\] ")
    packet = re.compile(rb"v=1 receipt=([1-9][0-9]?) seq=(0|[1-9][0-9]?) "
                        rb"stage=(entry|storage_begin|storage_end|storage_unavailable|selection_begin|selection_end|"
                        rb"legacy|home_init|connections_begin|connections_end|connections_failed|save_element|"
                        rb"render_returned|connection_name_(?:click|focus|blur|input)|unknown|cap) "
                        rb"elapsedMs=(-1|0|[1-9][0-9]{0,5}) end=1")
    anchor = None
    for line in lines[:result["scanLineLimit"]]:
        emitter = header.match(line)
        inherited = False
        if emitter:
            anchor = None
            payload = line[emitter.end():]
            context = (emitter[1], emitter[2])
        elif anchor and line.startswith(marker):
            payload, context, inherited = line, anchor, True
        else:
            anchor = None
            if marker in line:
                result["malformed"] = min(33, result["malformed"] + 1)
            continue
        if not payload.startswith(marker):
            continue
        match = packet.fullmatch(payload[len(marker):]) if len(line) <= result["lineLimitBytes"] else None
        valid = False
        if match:
            receipt, seq, stage, elapsed = int(match[1]), int(match[2]), match[3].decode("ascii"), int(match[4])
            valid = (receipt <= 33 and elapsed <= 600_000 and
                     ((stage in ("unknown", "cap") and seq == 0 and elapsed == -1
                       and (stage == "cap" or receipt <= 32)) or
                      (stage not in ("unknown", "cap") and receipt <= 32 and 1 <= seq <= 16)))
        if not valid:
            anchor = None
            result["malformed"] = min(33, result["malformed"] + 1)
            continue
        # Only a complete valid first line anchors adjacent complete fixed lines
        # in the same Logger packet. Never reconstruct a line across a cut/chunk.
        anchor = context if stage not in ("unknown", "cap") else None
        record = {"pid": int(context[1]), "receipt": receipt, "seq": seq, "stage": stage,
                  "elapsedMs": None if elapsed == -1 else elapsed,
                  "osLogTime": context[0].decode("ascii"), "inheritedHeader": inherited}
        if len(result["records"]) < result["recordLimit"]:
            result["records"].append(record)
        else:
            result["dropped"] = min(33, result["dropped"] + 1)
        if stage == "cap":
            result["capStatus"] = "observed"
    if len(json.dumps({"targetAppExit": {"bootstrapWitness": result}}, indent=2).encode()) > result["savedByteLimit"]:
        result["records"] = []
        result["errorType"] = "ProjectionBoundExceeded"
    return result


# watchdog-diagnostics-begin
WATCHDOG_REPORT_BYTES = 4 * 1024**2


def watchdog_time(value, *, explicit_utc=False):
    if not isinstance(value, str) or len(value) > 48:
        raise ValueError("Unknown native timestamp")
    if explicit_utc:
        return datetime.strptime(value, "%Y-%m-%d %H:%M:%S.%f").replace(tzinfo=timezone.utc)
    result = datetime.strptime(value, "%Y-%m-%d %H:%M:%S.%f %z")
    return result.astimezone(timezone.utc)


def _watchdog_witness(prefix, tail, *, explicit_utc, partial):
    """One original-process scene watchdog, from the existing native log query."""
    if (explicit_utc is not True or partial is not False or type(prefix) is not bytes or type(tail) is not bytes
            or len(prefix) > 2 * 1024**2 or len(tail) > 512 * 1024):
        return None
    native, watchdogs, exits = {}, {}, {}
    header = re.compile(rb"^(\d{4}-\d\d-\d\d \d\d:\d\d:\d\d\.\d{3,6})\s+(?:Df|E|D|I|F|A)\s+(.+)$")
    app = re.compile(rb"app<com\.takosumi\.fixture\.simulator(?:\(\(null\)\))?>:([1-9][0-9]{0,8})")
    for chunk, data in enumerate((prefix, tail)):
        # A cut chunk cannot acquire a previous line's header or identity.
        for index, line in enumerate(data.splitlines(keepends=True)[:16384]):
            if (chunk == 1 and index == 0) or not line.endswith(b"\n") or len(line) > 4096:
                continue
            match = header.fullmatch(line.rstrip(b"\r\n"))
            if not match:
                continue
            stamp, text = match[1].decode("ascii"), match[2]
            emitted = re.match(rb"Takosumi\[([1-9][0-9]{0,8}):[0-9a-f]+\]\s+\[com\.takosumi\.fixture\.simulator(?::app|\.navigation:navigation)\]\s+", text)
            if emitted:
                pid = int(emitted[1]); native[pid] = min(stamp, native.get(pid, stamp))
            if (re.match(rb"SpringBoard\[[0-9]+:[0-9a-f]+\]\s+\[com\.apple\.FrontBoard:Watchdog\]", text)
                    and b"Watchdog termination request provided:" in text and b"scene-update" in text
                    and b"0x8badf00d" in text.lower()):
                pids = {int(v) for v in app.findall(text)}
                if len(pids) == 1:
                    watchdogs.setdefault(pids.pop(), set()).add(stamp)
            if (re.match(rb"launchd_sim\[[1-9][0-9]{0,8}:[0-9a-fA-Fx]+\]\s+", text)
                    and b"exited with exit reason (namespace: 10 code: 0x8badf00d)" in text.lower()):
                found = re.search(rb"UIKitApplication:com\.takosumi\.fixture\.simulator\[[0-9a-f]+\]\[rb-legacy\] \[([1-9][0-9]{0,8})\]:\]", text)
                if found:
                    exits.setdefault(int(found[1]), set()).add(stamp)
    if len(watchdogs) != 1:
        return None
    pid, times = next(iter(watchdogs.items()))
    if len(times) != 1 or pid not in native or len(exits.get(pid, ())) != 1:
        return None
    first, event, ended = native[pid], next(iter(times)), next(iter(exits[pid]))
    if not watchdog_time(first, explicit_utc=True) <= watchdog_time(event, explicit_utc=True) <= watchdog_time(ended, explicit_utc=True):
        return None
    return {"pid": pid, "firstNativeUTC": first, "watchdogUTC": event, "exitUTC": ended,
            "clockDomain": "owned_simulator_os_log_explicit_UTC", "snapshotOnly": True}


def watchdog_witness(prefix, tail, *, explicit_utc, partial):
    try:
        return _watchdog_witness(prefix, tail, explicit_utc=explicit_utc, partial=partial)
    except Exception:
        return None  # This sibling projection cannot suppress old nav/screen retention.


def watchdog_install_context(container, data, udid, executable, load_commands):
    try:
        if str(uuid.UUID(udid)).upper() != udid or data.parent.name != udid or data.name != "data":
            return None
        relative = str((container / executable).relative_to(data))
        if not re.fullmatch(r"Containers/Bundle/Application/[A-Fa-f0-9-]{36}/[^/]+\.app/[^/]+", relative):
            return None
        if not isinstance(load_commands, str) or len(load_commands) > 65536:
            return None
        blocks = re.split(r"(?m)^Load command [0-9]+\n", load_commands)
        values = [re.findall(r"(?m)^\s+uuid ([A-Fa-f0-9-]{36})\s*$", b)
                  for b in blocks if re.search(r"(?m)^\s+cmd LC_UUID\s*$", b)]
        if len(values) != 1 or len(values[0]) != 1:
            return None
        if Path(executable).name != executable or not executable:
            return None
        uuid.UUID(Path(relative).parts[3])
        image = str(uuid.UUID(values[0][0]))
        return {"udid": udid, "bundleIdentifier": BUNDLE, "executable": executable,
                "relativeExecutableSHA256": hashlib.sha256(relative.encode()).hexdigest(), "imageUUID": image}
    except (ValueError, TypeError):
        return None


def watchdog_context_from_saved_install(path, device, udid, executable, load_commands):
    # Consume only the stdout already validated by installed_app. No extra
    # get_app_container or guest call, and no raw path survives the projection.
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd, "rb") as source:
            before = os.fstat(source.fileno())
            if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1 or not 0 < before.st_size <= 4096:
                return None
            raw = source.read(4097)
            after = os.fstat(source.fileno())
        if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns):
            return None
        text = raw.decode("utf-8").strip()
        if "\n" in text or "\r" in text or not text.startswith("/") or any(p in (".", "..") for p in text.split("/")):
            return None
        return watchdog_install_context(Path(text), Path(device["dataPath"]), udid, executable, load_commands)
    except Exception:
        return None


def watchdog_report(raw, context, witness, rejections=None):
    """Closed snapshot projection; never retain raw report/ASI/registers/paths."""
    def reject(category):
        if rejections is not None:
            rejections[category] = min(33, rejections.get(category, 0) + 1)
        return None
    if not context or not witness or type(raw) is not bytes or len(raw) > WATCHDOG_REPORT_BYTES:
        return reject("unavailable_or_bound")
    try:
        def unique(pairs):
            value = {}
            for key, item in pairs:
                if key in value:
                    raise ValueError("Duplicate report key")
                value[key] = item
            return value
        first, rest = raw.decode("utf-8").split("\n", 1)
        head = json.loads(first, object_pairs_hook=unique); body = json.loads(rest, object_pairs_hook=unique)
        if head.get("bundleID") != BUNDLE or body.get("bundleInfo", {}).get("CFBundleIdentifier") != BUNDLE:
            return reject("other_bundle")
        if type(body.get("pid")) is int and body["pid"] != witness["pid"]:
            return reject("other_pid")
        if (head.get("bug_type") != "309" or head.get("bundleID") != BUNDLE
                or body.get("bundleInfo", {}).get("CFBundleIdentifier") != BUNDLE
                or context["bundleIdentifier"] != BUNDLE or body.get("procName") != context["executable"]
                or type(body.get("pid")) is not int or body["pid"] != witness["pid"]
                or str(uuid.UUID(head["incident_id"])) != str(uuid.UUID(body["incident"]))):
            return reject("unsupported_or_inconsistent")
        launch, captured = watchdog_time(body["procLaunch"]), watchdog_time(body["captureTime"])
        if not (launch <= watchdog_time(witness["firstNativeUTC"], explicit_utc=True)
                <= watchdog_time(witness["watchdogUTC"], explicit_utc=True) <= captured
                <= watchdog_time(witness["exitUTC"], explicit_utc=True)):
            return reject("generation_or_interval")
        path = body["procPath"]
        if not isinstance(path, str) or len(path) > 2048 or not path.startswith("/") or any(v in (".", "..") for v in path.split("/")):
            return reject("unsupported_or_inconsistent")
        needle = "/Devices/" + context["udid"] + "/data/"
        if path.count(needle) != 1:
            return reject("other_device")
        relative = path.split(needle, 1)[1]
        if hashlib.sha256(relative.encode()).hexdigest() != context["relativeExecutableSHA256"]:
            return reject("other_install")
        termination = body.get("termination", {})
        code = termination.get("code")
        if type(code) is not int or code != 0x8badf00d or termination.get("namespace") not in ("FRONTBOARD", "SPRINGBOARD"):
            return reject("unsupported_or_inconsistent")
        if not isinstance(termination.get("indicator"), str) or len(termination["indicator"]) > 4096 or "scene-update" not in termination["indicator"]:
            return reject("unsupported_or_inconsistent")
        images = body.get("usedImages")
        if not isinstance(images, list) or not 1 <= len(images) <= 512:
            return reject("unsupported_or_inconsistent")
        main = [i for i, image in enumerate(images) if image.get("path") == path and
                str(uuid.UUID(image.get("uuid", ""))) == context["imageUUID"]]
        if len(main) != 1:
            return reject("other_binary")
        threads = body.get("threads")
        if not isinstance(threads, list) or len(threads) > 256:
            return reject("unsupported_or_inconsistent")
        selected = [t for t in threads if t.get("queue") == "com.apple.main-thread"]
        report = {"pid": body["pid"], "incident": str(uuid.UUID(body["incident"])),
                  "imageUUID": context["imageUUID"], "reportSHA256": hashlib.sha256(raw).hexdigest(),
                  "association": "native_watchdog_generation_and_owned_install", "snapshotOnly": True,
                  "threadRole": "unknown", "stackStatus": "unknown", "frames": [], "framesTruncated": False}
        if len(selected) != 1:
            return report
        frames = selected[0].get("frames")
        if not isinstance(frames, list) or not frames or len(frames) > 4096:
            return report
        report["threadRole"] = "main_queue"
        report["framesTruncated"] = len(frames) > 32
        for frame in frames[:32]:
            index, offset = frame.get("imageIndex"), frame.get("imageOffset")
            if type(index) is not int or not 0 <= index < len(images) or type(offset) is not int or not 0 <= offset < 2**64:
                return reject("unsupported_or_inconsistent")
            symbol = frame.get("symbol")
            if (not isinstance(symbol, str) or not re.fullmatch(r"[A-Za-z_$\[<+\-][A-Za-z0-9_.$<>\[\](): +,*&~\-]{0,239}", symbol)
                    or re.search(r"(?i)authorization|cookie|token|secret|password|bearer", symbol)):
                symbol = None
            report["frames"].append({"image": "app" if index == main[0] else "other",
                                     "imageOffset": offset, "symbol": symbol})
        report["stackStatus"] = "reported_main_queue_snapshot"
        return report
    except Exception:
        return reject("unsupported_or_inconsistent")


def watchdog_archive_name(name):
    if name.startswith("./"):
        name = name[2:]  # Standard relative tar/zip prefix; never a filesystem extraction.
    parts = name.replace("\\", "/").split("/")
    if (not name or len(name) > 1024 or name.startswith(("/", "\\")) or "\\" in name
            or any(p in (".", "..") for p in parts) or len(parts) > 13):
        raise ValueError("Diagnostic member path")
    return name.rstrip("/")


def watchdog_report_bytes(source, name, budget, deadline):
    """Direct IPS or one ZIP/tar/gzip container; selected bytes only, never extractall."""
    def admit(size):
        if runtime.time.monotonic() > deadline or not 0 <= size <= WATCHDOG_REPORT_BYTES or budget[0] <= 0 or budget[1] < size:
            raise ValueError("Diagnostic selection bound")
        budget[0] -= 1; budget[1] -= size
    if name.endswith(".ips"):
        source.seek(0, os.SEEK_END); admit(source.tell()); source.seek(0)
        data = source.read(WATCHDOG_REPORT_BYTES + 1)
        if len(data) > WATCHDOG_REPORT_BYTES:
            raise ValueError("Diagnostic report bound")
        yield data
        return
    class TimedInput:
        def read(self, count=-1):
            if runtime.time.monotonic() > deadline:
                raise ValueError("Diagnostic parse budget")
            if count < 0:
                position = source.tell(); source.seek(0, os.SEEK_END)
                count = source.tell() - position; source.seek(position)
            if count > 256 * 1024**2:
                raise ValueError("Diagnostic archive read bound")
            return source.read(count)
        def seek(self, *args):
            if runtime.time.monotonic() > deadline:
                raise ValueError("Diagnostic parse budget")
            return source.seek(*args)
        def tell(self):
            return source.tell()
        def seekable(self):
            return True
    archive_source = TimedInput()
    archive_suffixes = (".zip", ".tar", ".tgz", ".tar.gz")
    if name.endswith(".zip"):
        # Bound central metadata before zipfile allocates its member table.
        source.seek(0, os.SEEK_END); size = source.tell()
        source.seek(max(0, size - 65557)); tail = source.read(65557)
        position = tail.rfind(b"PK\x05\x06")
        if position < 0 or len(tail) - position < 22:
            raise ValueError("Diagnostic ZIP header")
        _, disk, central_disk, disk_count, count, central_size, offset, comment = struct.unpack("<4s4H2LH", tail[position:position+22])
        if (disk or central_disk or disk_count != count or count > 4096 or central_size > 2 * 1024**2
                or offset + central_size > size or position + 22 + comment != len(tail)):
            raise ValueError("Diagnostic ZIP metadata bound")
        eocd_offset = size - len(tail) + position
        if eocd_offset >= 20:
            source.seek(eocd_offset - 20)
            if source.read(4) == b"PK\x06\x07":
                raise ValueError("ZIP64 diagnostic metadata unknown")
        # ZipFile may otherwise overwrite the classic bounds from ZIP64, or
        # build more entries than the EOCD declares. Walk the small classic
        # central region first; do not let table construction own this bound.
        if offset + central_size != eocd_offset:
            raise ValueError("Diagnostic ZIP central offset")
        source.seek(offset)
        central = source.read(central_size)
        if len(central) != central_size:
            raise ValueError("Partial diagnostic ZIP central metadata")
        position, actual_count = 0, 0
        while position < len(central):
            if runtime.time.monotonic() > deadline:
                raise ValueError("Diagnostic parse budget")
            if len(central) - position < 46 or central[position:position+4] != b"PK\x01\x02":
                raise ValueError("Diagnostic ZIP central framing")
            name_bytes, extra_bytes, comment_bytes = struct.unpack_from("<3H", central, position + 28)
            position += 46 + name_bytes + extra_bytes + comment_bytes
            actual_count += 1
            if position > len(central) or actual_count > 4096:
                raise ValueError("Diagnostic ZIP actual member bound")
        if actual_count != count:
            raise ValueError("Diagnostic ZIP declared count mismatch")
        source.seek(0)
        with zipfile.ZipFile(archive_source) as archive:
            members = archive.infolist()
            if len(members) != count:
                raise ValueError("Diagnostic ZIP member count")
            seen, total = set(), 0
            for member in members:
                if runtime.time.monotonic() > deadline:
                    raise ValueError("Diagnostic parse budget")
                name = watchdog_archive_name(member.filename); mode = stat.S_IFMT(member.external_attr >> 16)
                total += member.file_size
                if (name in seen or member.flag_bits & 1 or mode not in (0, stat.S_IFREG, stat.S_IFDIR)
                        or member.file_size > 256 * 1024**2 or total > 512 * 1024**2):
                    raise ValueError("Diagnostic ZIP member bound/type")
                seen.add(name)
                if not member.is_dir() and name.endswith(archive_suffixes):
                    raise ValueError("Nested diagnostic archive unknown")
                if not member.is_dir() and name.endswith(".ips"):
                    admit(member.file_size)
                    yield archive.read(member)  # CRC checked only for selected IPS members.
        return
    if name.endswith((".tar", ".tgz", ".tar.gz")):
        class DecodedBudget:
            def __init__(self, stream):
                self.stream, self.remaining = stream, 512 * 1024**2
            def read(self, count):
                if count < 0:
                    raise ValueError("Unbounded diagnostic stream read")
                if runtime.time.monotonic() > deadline:
                    raise ValueError("Diagnostic parse budget")
                data = self.stream.read(min(count, self.remaining + 1))
                self.remaining -= len(data)
                if self.remaining < 0:
                    raise ValueError("Diagnostic decoded bound")
                return data
        decoded = gzip.GzipFile(fileobj=archive_source) if name.endswith((".tgz", ".tar.gz")) else archive_source
        limited = DecodedBudget(decoded)
        try:
            with tarfile.open(fileobj=limited, mode="r|") as archive:
                seen, total = set(), 0
                for count, member in enumerate(archive, 1):
                    if count > 4096:
                        raise ValueError("Diagnostic tar member bound")
                    if member.isdir() and member.name in (".", "./"):
                        continue
                    name = watchdog_archive_name(member.name); total += member.size
                    if (count > 4096 or name in seen or not (member.isdir() or member.isfile())
                            or member.size > 256 * 1024**2 or total > 512 * 1024**2):
                        raise ValueError("Diagnostic tar member bound/type")
                    seen.add(name)
                    if member.isfile() and name.endswith(archive_suffixes):
                        raise ValueError("Nested diagnostic archive unknown")
                    if member.isfile() and name.endswith(".ips"):
                        admit(member.size)
                        report = archive.extractfile(member)
                        if report is None:
                            raise ValueError("Missing diagnostic report")
                        yield report.read(WATCHDOG_REPORT_BYTES + 1)
            # Consume bounded padding/trailer too, so a gzip CRC/size failure
            # cannot be hidden by tar's end marker after a selected report.
            while limited.read(65536):
                pass
        finally:
            if decoded is not archive_source:
                decoded.close()
        return
    raise ValueError("Unknown diagnostic container")


def preserve_watchdog_diagnostics(out, scratch, original, context):
    value = {"diagnosticOnly": True, "status": "unknown", "absenceIsUnknown": True,
             "snapshotOnly": True, "exportEntryLimit": 4096, "exportByteLimit": 512 * 1024**2,
             "reportByteLimit": WATCHDOG_REPORT_BYTES, "reportLimit": 32}
    try:
        value["stage"] = "eligibility"
        witness = original.get("targetAppExit", {}).get("watchdogWitness")
        if original.get("functionalPassed") is not False or not witness or not context:
            return value
        if any(v["log"] == str(out / "xcodebuild.log") for v in runtime.INTERRUPTED_COMMANDS) or runtime.UNREAPED_COMMANDS:
            return value
        bundle = out / "Tests.xcresult"
        runtime.diagnostic_export_tree(bundle, entry_limit=16384, byte_limit=1024**3)
        directory = scratch / ("watchdog-export-" + uuid.uuid4().hex)
        directory.mkdir(mode=0o700)
        anchor = directory.stat()
        value["stage"] = "export"
        deadline = runtime.time.monotonic() + 60
        runtime.command(["xcrun", "xcresulttool", "export", "diagnostics", "--path", str(bundle),
                         "--output-path", str(directory / "diagnostics")], directory / "export.log",
                        timeout=60, read_output=False, diagnostic_export_root=directory)
        if runtime.UNREAPED_COMMANDS:
            return value
        files = runtime.diagnostic_export_tree(directory)
        value["containers"] = {"ips": 0, "zip": 0, "tar": 0, "tar_gzip": 0}
        reports, total, count, rejections = {}, 0, 0, {}
        budget = [32, 32 * 1024**2]
        value["stage"] = "read"
        for parts, identity in files:
            if not parts[-1].endswith((".ips", ".zip", ".tar", ".tgz", ".tar.gz")):
                if parts[-1].endswith((".gz", ".xz", ".bz2", ".zst", ".7z", ".crash")):
                    value["outcome"] = "unsupported_container_or_report"
                    return value
                continue
            kind = ("ips" if parts[-1].endswith(".ips") else "zip" if parts[-1].endswith(".zip") else
                    "tar" if parts[-1].endswith(".tar") else "tar_gzip")
            value["containers"][kind] += 1
            # Reopen each component without following links; bind the stream
            # to the scan inode. No archive member becomes a filesystem path.
            with ExitStack() as stack:
                fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
                stack.callback(os.close, fd)
                if (os.fstat(fd).st_dev, os.fstat(fd).st_ino) != (anchor.st_dev, anchor.st_ino):
                    return value
                for part in parts[:-1]:
                    fd = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
                    stack.callback(os.close, fd)
                fd = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=fd)
                stack.callback(os.close, fd)
                before = os.fstat(fd)
                if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != identity or before.st_nlink != 1:
                    return value
                with os.fdopen(os.dup(fd), "rb") as source:
                    for raw in watchdog_report_bytes(source, parts[-1], budget, deadline):
                        count += 1; total += len(raw)
                        if count > 32 or len(raw) > WATCHDOG_REPORT_BYTES or total > 32 * 1024**2:
                            return value
                        report = watchdog_report(raw, context, witness, rejections)
                        if report:
                            previous = reports.get(report["incident"])
                            if previous is not None and previous != report:
                                return value
                            reports[report["incident"]] = report
                after = os.fstat(fd)
                if (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns) != identity:
                    return value
        if runtime.time.monotonic() > deadline:
            return value
        value.update(ipsReportsRead=count, rejections=rejections)
        unsafe = set(rejections) - {"other_bundle", "other_pid", "other_device", "other_install", "other_binary", "generation_or_interval"}
        if len(reports) != 1 or unsafe:
            return value
        value.update(status="retained", report=next(iter(reports.values())))
        data = (json.dumps(value, indent=2) + "\n").encode()
        if len(data) > 32768:
            return {"diagnosticOnly": True, "status": "unknown", "absenceIsUnknown": True}
        # The existing final result.json save owns delivery; no duplicate report
        # file, raw IPS, SDK archive or stdout is copied into evidence.
        return value
    except Exception as error:
        value["outcome"] = ({"Nested diagnostic archive unknown": "unsupported_nested_container",
                             "ZIP64 diagnostic metadata unknown": "unsupported_zip64",
                             "Unknown diagnostic container": "unsupported_container"}.get(str(error), "invalid_or_bound")
                            if isinstance(error, ValueError) else
                            "timeout" if isinstance(error, subprocess.TimeoutExpired) else
                            "io" if isinstance(error, OSError) else
                            "export_rejected" if isinstance(error, RuntimeError) else "unknown")
        # Optional export/parse/storage failures never replace the original UI
        # failure. Existing command retirement/scratch retention still applies.
        value["status"] = "unknown"
        value.pop("report", None)
        return value
# watchdog-diagnostics-end


def require_target_log_owner(path, udid, device, name, selected_runtime, timeout):
    inventory = json.loads(runtime.command(["xcrun", "simctl", "list", "devices", "--json"], path, timeout=timeout))
    current = runtime.owned_device(inventory, udid, name, selected_runtime)
    require(current.get("state") == "Booted" and current.get("dataPath") == device.get("dataPath")
            and current.get("logPath") == device.get("logPath"), "Owned Simulator changed before target log read")


def filtered_navigation_witness(scratch, udid, device, name, selected_runtime, *, owner_already_checked=False):
    """Read only this owned App's fixture navigation producer."""
    result = {"diagnosticOnly": True, "absenceIsUnknown": True, "status": "unknown", "queryReturned": False}
    deadline = runtime.time.monotonic() + FILTERED_NAVIGATION_WAIT
    try:
        if not owner_already_checked:
            require_target_log_owner(scratch / "before-filtered-navigation.log", udid, device, name, selected_runtime, timeout=2)
        remaining = deadline - runtime.time.monotonic()
        if remaining <= 0:
            return result
        raw = scratch / "filtered-navigation-raw.log"
        predicate = '(process == "Takosumi") AND (subsystem == "com.takosumi.fixture.simulator.navigation")'
        try:
            runtime.command(["xcrun", "simctl", "spawn", udid, "log", "show", "--style", "compact",
                             "--timezone", "UTC", "--last", "10m", "--predicate", predicate],
                            raw, timeout=remaining, read_output=False)
            result["queryReturned"] = True
        except subprocess.TimeoutExpired:
            result["partial"] = True
            if runtime.UNREAPED_COMMANDS:
                return result
        # Keep raw output in scratch. Freeze the observed length and feed only
        # a bounded tail into the same closed projection as the broad read.
        with os.fdopen(os.open(raw, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as source:
            info = os.fstat(source.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
                return result
            size = info.st_size
            source.seek(max(0, size - NAVIGATION_LOG_LIMIT))
            data = source.read(min(size, NAVIGATION_LOG_LIMIT))
            after = os.fstat(source.fileno())
            if len(data) != min(size, NAVIGATION_LOG_LIMIT) or (size, info.st_mtime_ns) != (after.st_size, after.st_mtime_ns):
                result["partial"] = True
                return result
        partial = not result["queryReturned"] or size > len(data)
        result.update(navigation_witness(data), partial=partial, sourceBytes=size, keptBytes=len(data))
        if result["records"]:
            result["status"] = "partial" if partial else "retained"
    except Exception:
        # The optional read, ownership check and parser never change the
        # broad receipt or replace the original XCTest failure.
        pass
    return result


def preserve_app_exit(out, scratch, udid, device, name, selected_runtime, *, navigation_first=False):
    """Optional, bounded target logs from this exact newly created Simulator."""
    result = {"diagnosticOnly": True, "udid": udid, "bundleIdentifier": BUNDLE,
              "scope": "Owned Simulator only; Takosumi process or private fixture bundle events",
              "excludedSubsystems": ["com.apple.dt.xctest"]}
    try:
        # Recheck inventory before spawn. Never query a host-wide log store or
        # a Simulator selected by its display name alone.
        require_target_log_owner(out / "before-target-logs.log", udid, device, name, selected_runtime, timeout=20)
        # No custom/remote process name or predicate input. The bundle is the
        # preinstalled private Tauri App, not the minimal XCTest driver.
        # XCTest's verbose AX traversal can consume the query's entire budget
        # before it reaches the failure time. Its AX snapshots and diagnostics
        # remain in xcodebuild.log and the separately exported test attachments.
        predicate = ('((process == "Takosumi") OR (eventMessage CONTAINS "com.takosumi.fixture.simulator")) '
                     'AND (subsystem != "com.apple.dt.xctest")')
        raw = scratch / "target-app-exit-raw.log"
    except Exception as error:
        result["errorType"] = type(error).__name__
        return result
    if navigation_first:
        # An XCTest assertion already identifies a functional failure. Start
        # the narrow capture at the just-verified Simulator identity, before
        # the broad query can delay or prevent it.
        result["filteredNavigationWitness"] = filtered_navigation_witness(
            scratch, udid, device, name, selected_runtime, owner_already_checked=True)
        if runtime.UNREAPED_COMMANDS:
            return result
    timed_out = False
    try:
        runtime.command(["xcrun", "simctl", "spawn", udid, "log", "show", "--style", "compact",
                         "--timezone", "UTC", "--last", "10m", "--predicate", predicate], raw, timeout=15, read_output=False)
        result["queryReturned"] = True
    except Exception as error:
        # command() handles its direct child before raising. A retained tail
        # is only a partial snapshot; interrupted-command scratch stays held.
        result.update(queryReturned=False, errorType=type(error).__name__)
        timed_out = isinstance(error, subprocess.TimeoutExpired)
    try:
        with raw.open("rb") as source:
            size = os.fstat(source.fileno()).st_size
            prefix = None
            try:
                prefix = source.read(min(size, 2 * 1024 * 1024))
                result["screenWitness"] = screen_witness(io.BytesIO(prefix), size, partial=not result["queryReturned"])
            except Exception as error:
                result["screenWitness"] = {"diagnosticOnly": True, "absenceIsUnknown": True,
                                           "errorType": type(error).__name__}
            try:
                result["bootstrapWitness"] = bootstrap_witness(prefix, size, partial=not result["queryReturned"])
            except Exception as error:
                result["bootstrapWitness"] = {"diagnosticOnly": True, "absenceIsUnknown": True,
                                              "errorType": type(error).__name__}
            source.seek(max(0, size - 512 * 1024))
            # Freeze the size observed above even if an interrupted CLI's
            # descendant is still appending. This is not a drain receipt.
            data = source.read(min(size, 512 * 1024))
        with (out / "target-app-exit.log").open("xb") as target:
            target.write(data)
        result["watchdogWitness"] = watchdog_witness(prefix, data, explicit_utc=True, partial=not result["queryReturned"])
        result.update(sourceBytes=size, keptBytes=len(data), truncated=size > len(data),
                      sha256=hashlib.sha256(data).hexdigest(), partial=not result["queryReturned"])
        try:
            result["navigationWitness"] = navigation_witness(data)
        except Exception as error:
            result["navigationWitness"] = {"diagnosticOnly": True, "errorType": type(error).__name__}
    except Exception as error:
        # Missing/failed diagnostics never replace the original test failure.
        result["retentionErrorType"] = type(error).__name__
    if (not navigation_first and timed_out and not runtime.UNREAPED_COMMANDS and
            any(item.get("log") == str(raw) and item.get("reaped") is True for item in runtime.INTERRUPTED_COMMANDS)):
        result["filteredNavigationWitness"] = filtered_navigation_witness(scratch, udid, device, name, selected_runtime)
    return result


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


def failure_job_summary(data):
    """Closed projection of an existing bounded XCTest log, never a verdict."""
    result = {"kind": "takosumi.ios-web-home-failure@v1", "category": "unknown",
              "sourceLine": None, "started": None, "failed": None, "passed": None}
    if type(data) is not bytes or not data or len(data) > 256 * 1024:
        return result
    try:
        text = data.decode("utf-8")
    except UnicodeError:
        return result
    if "\x00" in text:
        return result
    identity = f"-[WebHomeUITests.WebHomeUITests {METHOD}]"
    cases = []
    for line in text.splitlines():
        if not line.startswith("Test Case '"):
            continue
        match = re.fullmatch(r"Test Case '([^']+)' (started\.|(?:passed|failed) \([0-9]{1,6}(?:\.[0-9]{1,6})? seconds\)\.)", line)
        if not match or match[1] != identity:
            return result
        cases.append(match[2].split()[0].rstrip("."))
    counts = {key: cases.count(key) for key in ("started", "failed", "passed")}
    result.update({key: min(value, 2) for key, value in counts.items()})
    if not cases:
        result["category"] = "xctest_not_started"
    elif cases == ["started", "failed"]:
        result["category"] = "xctest_failure"
        # Only current public fixture assertion/call sites, never arbitrary
        # filenames, messages, titles, URLs or values from XCTest diagnostics.
        allowed = {"15", "18", "22", "30", "44", "51", "60", "67", "69", "71", "82", "89", "95", "100",
                   "107", "111", "127", "129", "161", "165", "187", "189", "190", "195", "204", "207",
                   "212", "215", "226", "233", "257"}
        sources = []
        for line in text.splitlines():
            location, separator, _ = line.partition(f": error: {identity} : ")
            if separator:
                source, _, number = location.rpartition(":")
                if (source == "WebHomeUITests.swift" or source.endswith(
                        "/native-tests/ios/web-home-fixture/WebHomeUITests.swift")) and number in allowed:
                    sources.append(int(number))
                else:
                    sources.append(None)
        if len(sources) == 1:
            result["sourceLine"] = sources[0]
    elif cases == ["started", "passed"]:
        result["category"] = "post_xctest"
    return result


def completed_xcode_failure_summary(out):
    """Snapshot only this completed command's regular, bounded owned log."""
    path = out / "xcodebuild.log"
    try:
        if (any(item.get("log") == str(path) for item in runtime.INTERRUPTED_COMMANDS)
                or runtime.UNREAPED_COMMANDS):
            return None
        with ExitStack() as stack:
            directory = os.open(out, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            stack.callback(os.close, directory)
            fd = os.open("xcodebuild.log", os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                         dir_fd=directory)
            stack.callback(os.close, fd)
            before = os.fstat(fd)
            if (not stat.S_ISREG(before.st_mode) or before.st_nlink != 1 or
                    before.st_size < 1 or before.st_size > 256 * 1024):
                return None
            with os.fdopen(os.dup(fd), "rb") as source:
                data = source.read(256 * 1024 + 1)
            after = os.fstat(fd)
            identity = lambda value: (value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns)
            if len(data) != before.st_size or identity(before) != identity(after):
                return None
            return failure_job_summary(data)
    except Exception:
        # Optional evidence cannot replace the original XCTest or CLI failure.
        return None


def emit_failure_job_summary(out, result, completed_xcode=None):
    """One optional line after final result/cleanup; failures cannot replace refusal."""
    try:
        if type(result) is not dict or result.get("passed") is not False:
            return
        summary = None
        known = result.get("xctest")
        if (type(known) is dict and known.get("method") == METHOD and
                type(known.get("tests")) is int and known["tests"] == 1 and
                type(known.get("failures")) is int and known["failures"] == 0):
            summary = failure_job_summary(b"")
            summary.update(category="post_xctest", started=1, failed=0, passed=1)
        if summary is None and not result.get("activeOwnedCommands") and not any(
                item.get("log") == str(out / "xcodebuild.log")
                for item in result.get("interruptedCommands", [])):
            summary = completed_xcode
            if summary is None and result.get("interruptedCommands") == []:
                summary = completed_xcode_failure_summary(out)
        line = "IOS_WEB_HOME_FAILURE " + json.dumps(summary or failure_job_summary(b""), separators=(",", ":"))
        if len(line.encode("ascii")) <= 512:
            print(line, flush=True)
    except Exception:
        pass  # Diagnostic read/serialization/sink errors never replace the original verdict.


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
    runtime_file = BUILD / "swift-runtime-proof.json"
    require(runtime_file.is_file() and not runtime_file.is_symlink() and runtime_file.stat().st_size <= 8 * 1024**2,
            "Missing bounded SDK Swift runtime proof")
    swift_runtime = json.loads(runtime_file.read_text())
    require(swift_runtime.get("passed") is True and swift_runtime.get("bundleIdentifier") == BUNDLE,
            "SDK Swift search-route inspection did not pass")
    images = swift_runtime.get("images", [])
    require(images and images[0].get("path") == app_proof["executable"]
            and images[0].get("sha256") == app_proof["executableSHA256"], "Swift proof belongs to another App")
    for image in images:
        path = Path(image.get("path", ""))
        require(not path.is_absolute() and path.parts and ".." not in path.parts and digest(app / path) == image.get("sha256"),
                "Swift runtime image differs from the SDK proof")
    out = args.out.resolve()
    require(not out.is_relative_to(ROOT), "Evidence must stay outside the repository")
    out.mkdir(mode=0o700)
    shutil.copyfile(runtime_file, out / "swift-runtime-proof.json")
    scratch_parent = Path(os.environ.get("RUNNER_TEMP", os.environ.get("TMPDIR", "/root/dev/tmp")))
    require(not scratch_parent.resolve().is_relative_to(ROOT), "Private scratch must stay outside the repository")
    require(shutil.disk_usage(scratch_parent).free >= 8 * 1024**3, "Need 8 GiB free; no broad cleanup")
    result = {"passed": False, "scope": "Whole installed Tauri App, synthetic Web UI only",
              "source": source_hashes(), "app": app_proof, "installBudgetSeconds": INSTALL_WAIT,
              "cleanup": False, "functionalPassed": False}
    diagnostic_context = {}
    completed_xcode = None
    udid, created, servers, failure = None, None, [], None
    with runtime.owned_scratch(scratch_parent) as (scratch, scratch_state):
        try:
            checkout = runtime.command(["git", "-C", str(ROOT), "rev-parse", "HEAD", "HEAD^{tree}"],
                                       out / "checkout.log").splitlines()
            require(len(checkout) == 2 and all(re.fullmatch(r"[0-9a-f]{40}", v) for v in checkout), "Invalid checkout")
            result["checkout"] = {"head": checkout[0], "tree": checkout[1]}
            require(swift_runtime.get("head") == checkout[0] and swift_runtime.get("tree") == checkout[1],
                    "SDK Swift runtime proof belongs to another source checkout")
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
            runtime.import_simulator_ca_with_one_timeout_retry(out, udid, ca, result)
            runtime.command(["xcrun", "simctl", "install", udid, str(app)], out / "install.log", timeout=INSTALL_WAIT)
            result["installedBefore"] = installed_app(out, udid, created, app_proof, "before")
            diagnostic_context = watchdog_context_from_saved_install(out / "installed-before.log", created,
                udid, app_proof["executable"], images[0].get("loadCommands", ""))
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
            try:
                log = runtime.command(["xcodebuild", "test", "-project", str(scratch / "WebHomeDriver.xcodeproj"),
                                       "-scheme", "WebHomeDriver", "-destination", f"id={udid}",
                                       "-derivedDataPath", str(scratch / "derived"), "-resultBundlePath", str(out / "Tests.xcresult"),
                                       "-parallel-testing-enabled", "NO", "-maximum-concurrent-test-simulator-destinations", "1",
                                       "-only-testing:WebHomeUITests/WebHomeUITests/" + METHOD, "CODE_SIGNING_ALLOWED=NO"],
                                      out / "xcodebuild.log", timeout=600, env=env)
                result["xctest"] = require_ui_pass(log)
            except Exception:
                completed_xcode = completed_xcode_failure_summary(out)
                raise
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
                result["targetAppExit"] = preserve_app_exit(out, scratch, udid, created, name, selected_runtime,
                    navigation_first=(completed_xcode or {}).get("category") == "xctest_failure")
        finally:
            result["testAttachments"] = runtime.export_test_attachments(out)
            for server in servers:
                try:
                    server.stop()
                    runtime.save(out / (server.kind + "-events.json"), server.events)
                    if failure is not None:
                        result.setdefault("fixtureTransport", {"diagnosticOnly": True})[server.kind] = server.transport_summary()
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
            if failure is not None:
                result["watchdogDiagnostics"] = preserve_watchdog_diagnostics(out, scratch, result, diagnostic_context)
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
    emit_failure_job_summary(out, result, completed_xcode)
    require(result["passed"], "Whole App iOS Web-home fixture failed; inspect its original result.json")
    print("PASS: whole installed Tauri App connection, synthetic Web login/home, app, Back, profile switch, resume and reset")
    print("Disposable Simulator only; real IdP, physical devices, files/notifications and distribution remain separate")


if __name__ == "__main__":
    main()
