#!/usr/bin/env python3
"""Drive the actual installed Tauri Android App on one owned emulator."""
import argparse
from contextlib import contextmanager
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import signal
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import time
import uuid
import xml.etree.ElementTree as ET

from fixture_network import Overlay, PACKAGE
from owned_process import identity
from verify_app import verify_app

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
DRIVER = "com.takosumi.fixture.webhome"
CLASS = DRIVER + ".WebHomeOperationTest"
METHOD = "wholeAppConnectionsWebHomeAndResumeStayProfileBound"
SERIAL = "emulator-5582"
CASES = ["connection-web-login-home-back", "cross-origin-no-ipc", "warm-document-retained",
         "profile-auth-and-draft-isolated", "cold-process-and-profiles-restored",
         "cleared-profile-reset", "other-profile-preserved", "native-views-closed"]


def require(value, message):
    if not value:
        raise RuntimeError(message)


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# Both platforms exercise the same two HTTPS sites. The historical iOS CLI
# helper also supplies bounded direct-child waits and private CA generation;
# no Simulator/native behavior is called on Linux.
WEB = HERE.parent / "ios/web-home-fixture"
runtime = load("sumi_android_web_harness", WEB / "harness.py").runtime
Site = load("sumi_android_web_site", WEB / "web_site.py").Site


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


class FileCustody:
    """Bind restoration to plain private file inodes and their parent chain."""
    def __init__(self, path, anchor):
        require(path.is_absolute() and anchor.is_absolute() and path.is_relative_to(anchor)
                and ".." not in path.relative_to(anchor).parts, "Private artifact is outside its owner")
        self.path, self.anchor = path, anchor
        self.before = self.observe()

    def observe(self):
        values, current = {}, self.path
        while True:
            info = current.lstat()
            if current == self.path:
                require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1, "Plain unshared private artifact required")
            else:
                require(stat.S_ISDIR(info.st_mode) and not stat.S_ISLNK(info.st_mode), "Plain private artifact parents required")
            values[current] = (info.st_dev, info.st_ino)
            if current == self.anchor:
                return values
            current = current.parent

    @contextmanager
    def open(self, writable=False):
        require(self.observe() == self.before, "Private artifact custody changed")
        fd = os.open(self.path, (os.O_RDWR if writable else os.O_RDONLY) | os.O_NOFOLLOW)
        with os.fdopen(fd, "r+b" if writable else "rb") as stream:
            info = os.fstat(stream.fileno())
            require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1
                    and (info.st_dev, info.st_ino) == self.before[self.path]
                    and self.observe() == self.before, "Private artifact was replaced while opening")
            yield stream


def restore_apk(destination, backup, variant_hash, baseline_hash):
    with backup.open() as source, destination.open(writable=True) as target:
        require(hashlib.file_digest(source, "sha256").hexdigest() == baseline_hash
                and hashlib.file_digest(target, "sha256").hexdigest() == variant_hash,
                "Changed App or private backup; refusing overwrite")
        source.seek(0)
        target.seek(0)
        shutil.copyfileobj(source, target)
        target.truncate()
        target.flush()


def require_ui_pass(log, result):
    blocks = []
    current = {}
    for line in log.splitlines():
        field = re.fullmatch(r"INSTRUMENTATION_STATUS: (class|test|numtests|current)=(.*)", line.strip())
        if field:
            require(field[1] not in current, "Duplicate instrumentation field")
            current[field[1]] = field[2]
        code = re.fullmatch(r"INSTRUMENTATION_STATUS_CODE: (-?[0-9]+)", line.strip())
        if code:
            blocks.append((int(code[1]), current))
            current = {}
    expected = {"class": CLASS, "test": METHOD, "numtests": "1", "current": "1"}
    require(blocks == [(1, expected), (0, expected)] and not current,
            "Exactly one started and passed instrumentation case is required")
    require(re.findall(r"(?m)^OK \(([0-9]+) tests?\)\s*$", log) == ["1"]
            and re.findall(r"(?m)^INSTRUMENTATION_CODE: (-?[0-9]+)\s*$", log) == ["-1"]
            and "INSTRUMENTATION_FAILED" not in log, "Missing nonempty instrumentation success")
    require(result.get("passed") is True and result.get("cases") == CASES
            and result.get("diagnosticsSaved") is True, "Incomplete actual App UI result")
    before, after = result.get("oldPid", ""), result.get("newPid", "")
    require(re.fullmatch(r"[1-9][0-9]*", before) and re.fullmatch(r"[1-9][0-9]*", after)
            and before != after, "Cold resume did not replace the App process")
    documents = [result.get(key, "").removeprefix("Document ")
                 for key in ("beforeColdDocument", "afterColdDocument")]
    require(all(str(uuid.UUID(value)) == value.lower() for value in documents)
            and documents[0] != documents[1], "Cold resume did not create a new Web document")
    return {"tests": 1, "failures": 0, "class": CLASS, "method": METHOD}


def require_site_events(instance, editor):
    for user in ("Work", "Personal"):
        require(any(e["path"] == "/sign-in/" + user and e["user"] is None for e in instance)
                and any(e["path"] == "/" and e["user"] == user for e in instance),
                "Missing fresh profile login and authenticated home")
    require(sum(e["path"] == "/" and e["user"] is None for e in instance) >= 3
            and sum(e["path"] == "/sign-in/Work" and e["user"] is None for e in instance) >= 2
            and any(e["path"] == "/account" and e["user"] == "Work" for e in instance),
            "Missing cleared-profile login or actual Web management")
    require(sum(e["path"] == "/editor" for e in editor) >= 4
            and any(e["path"] == "/editor/page-two" for e in editor)
            and all(e["cookiePresent"] is False and e["user"] is None for e in editor),
            "Missing cross-origin navigation or instance cookie leaked to editor")


def receipt(value, normal):
    require(value.get("complete") is True and value.get("kernelNoChildren") is True
            and value.get("remaining") == [] and value.get("error") is None
            and value.get("cleanupErrors") == [] and value.get("nonFatalErrors") == [],
            "Emulator guard has no complete kernel drain proof")
    if normal:
        require(value.get("rootExit") == 0 and value.get("forcedCleanup") == 0,
                "Successful UI qualification requires normal emulator exit")


# stop-observation-begin
STOP_OBSERVATION_CAP = 16


def update_stop_observation(emulator, stage, *, phase=None, error=None, exit_code=None):
    if stage == "start":
        process = emulator.process
        pid = process.pid if process is not None else None
        birth = getattr(emulator, "birth", None)
        emulator.stop_observation = {"schema": "takosumi.emulator-stop-observation@v1", "diagnosticOnly": True,
            "guardPid": pid if type(pid) is int and pid > 0 else None,
            "guardStartTick": birth if type(birth) is int and birth >= 0 else None,
            "capacity": STOP_OBSERVATION_CAP, "truncated": False, "records": []}
    value = emulator.stop_observation
    if len(value["records"]) >= STOP_OBSERVATION_CAP:
        value["truncated"] = True
        return
    record = {"stage": stage}
    if phase is not None:
        record["phase"] = phase
    if exit_code is not None and type(exit_code) is int:
        record["exitCode"] = exit_code
    if error is not None:
        record["errorCategory"] = ("command_timeout" if isinstance(error, subprocess.TimeoutExpired) else
            "timeout" if isinstance(error, TimeoutError) else "missing" if isinstance(error, ProcessLookupError) else
            "permission" if isinstance(error, PermissionError) else "os" if isinstance(error, OSError) else
            "runtime" if isinstance(error, RuntimeError) else "other")
        errno = error.errno if isinstance(error, OSError) else None
        if type(errno) is int:
            record["errno"] = errno
    value["records"].append(record)


def observe_stop(emulator, stage, **values):
    try:
        update_stop_observation(emulator, stage, **values)
    except Exception:
        pass  # Metadata never replaces the original stop/cleanup exception.


def preserve_stop_observation(emulator, proof):
    try:
        value = emulator.stop_observation
        proof["emulatorStopObservation"] = {name: value[name] for name in (
            "schema", "diagnosticOnly", "guardPid", "guardStartTick", "capacity", "truncated")}
        proof["emulatorStopObservation"]["records"] = [dict(record) for record in value["records"][:STOP_OBSERVATION_CAP]]
    except Exception:
        pass  # An unavailable observer/sink is not a cleanup or restoration result.
# stop-observation-end


# sdk-candidate-witness-begin
SDK_CANDIDATE_WITNESS_CAP = 16


def update_sdk_candidate_witness(emulator, pid, start_tick):
    guard_pid, guard_birth = emulator.process.pid, emulator.birth
    if not (type(guard_pid) is int and guard_pid > 0 and type(guard_birth) is int and guard_birth >= 0
            and type(pid) is int and pid > 0 and type(start_tick) is int and start_tick >= 0):
        return
    value = getattr(emulator, "sdk_candidate_witness", None)
    if value is None:
        value = {"guardPid": guard_pid, "guardStartTick": guard_birth,
                 "truncated": False, "guardMismatch": False, "records": []}
        emulator.sdk_candidate_witness = value
    if (value["guardPid"], value["guardStartTick"]) != (guard_pid, guard_birth):
        value["guardMismatch"] = True
        return
    record = {"pid": pid, "startTick": start_tick}
    if record in value["records"]:
        return  # Coalesce repeated validations without an unbounded counter.
    if len(value["records"]) >= SDK_CANDIDATE_WITNESS_CAP:
        value["truncated"] = True
    else:
        value["records"].append(record)


def observe_sdk_candidate(emulator, pid, start_tick):
    try:
        update_sdk_candidate_witness(emulator, pid, start_tick)
    except Exception:
        pass  # Historical metadata never changes ownership validation.


def preserve_sdk_candidate_witness(emulator, proof):
    try:
        value = getattr(emulator, "sdk_candidate_witness", None)
        records = [] if value is None else [
            {"pid": record["pid"], "startTick": record["startTick"]}
            for record in value["records"][:SDK_CANDIDATE_WITNESS_CAP]]
        guard_pid, guard_birth = emulator.process.pid if emulator.process is not None else None, getattr(emulator, "birth", None)
        truncated = value is not None and value["truncated"]
        mismatch = value is not None and (value["guardMismatch"] or
            (value["guardPid"], value["guardStartTick"]) != (guard_pid, guard_birth))
        proof["emulatorSDKCandidateWitness"] = {
            "schema": "takosumi.emulator-sdk-candidate-witness@v1", "diagnosticOnly": True,
            "historicalOnly": True, "capacity": SDK_CANDIDATE_WITNESS_CAP,
            "guardPid": value["guardPid"] if value is not None else (guard_pid if type(guard_pid) is int and guard_pid > 0 else None),
            "guardStartTick": value["guardStartTick"] if value is not None else (guard_birth if type(guard_birth) is int and guard_birth >= 0 else None),
            "truncated": truncated, "guardMismatch": mismatch,
            "candidateStatus": "single_historical" if len(records) == 1 and not truncated and not mismatch else "unknown",
            "records": records}
    except Exception:
        pass  # Missing metadata or a refused sink cannot replace cleanup/errors.
# sdk-candidate-witness-end


class Emulator:
    def __init__(self, sdk, scratch, out, command):
        self.sdk, self.out, self.command = sdk, out, command
        self.adb = sdk / "platform-tools/adb"
        self.name = "sumi-whole-app-" + str(uuid.uuid4())
        self.env = dict(os.environ, ANDROID_AVD_HOME=str(scratch / "avd"))
        Path(self.env["ANDROID_AVD_HOME"]).mkdir()
        self.process = None
        self.fd = None
        self.proof = out / "emulator-guard.json"

    def start(self):
        self.command([str(self.adb), "start-server"], "adb-server", timeout=30)
        inventory = self.command([str(self.adb), "devices"], "before-devices")
        require(inventory.strip() == "List of devices attached", "Existing Android devices refused")
        for port in (5582, 5583):
            with socket.socket() as probe:
                probe.bind(("127.0.0.1", port))
        self.command([str(self.sdk / "cmdline-tools/latest/bin/avdmanager"), "create", "avd",
                      "--name", self.name, "--package", "system-images;android-36;google_apis;x86_64",
                      "--device", "pixel_7"], "avd-create", env=self.env, timeout=60)
        # This touch-input case must observe and dismiss the software IME.
        # Change only the fresh private AVD, never a host keyboard setting.
        config = Path(self.env["ANDROID_AVD_HOME"]) / (self.name + ".avd") / "config.ini"
        require(config.is_file() and not config.is_symlink(), "Owned AVD keyboard config is missing")
        rows = config.read_text().splitlines()
        keyboard = [i for i, row in enumerate(rows) if row.split("=", 1)[0].strip() == "hw.keyboard"]
        require(len(keyboard) <= 1, "Owned AVD keyboard config is ambiguous")
        if keyboard:
            rows[keyboard[0]] = "hw.keyboard=no"
        else:
            rows.append("hw.keyboard=no")
        config.write_text("\n".join(rows) + "\n")
        require(config.read_text().splitlines().count("hw.keyboard=no") == 1, "Owned AVD keyboard mode differs")
        (self.out / "keyboard-mode.json").write_text(json.dumps({
            "scope": "Fresh private whole-App AVD only", "hardwareKeyboard": False,
        }) + "\n")
        guard = ROOT / "src-tauri/target/debug/sumi-android-emulator-guard"
        require(guard.is_file() and not guard.is_symlink(), "Build the existing-source emulator guard first")
        self.output = (self.out / "emulator.log").open("xb")
        self.process = subprocess.Popen([str(guard), str(self.proof), str(self.sdk / "emulator/emulator"),
            "-avd", self.name, "-port", "5582", "-memory", "4096", "-no-window", "-no-audio",
            "-gpu", "swiftshader_indirect", "-no-boot-anim", "-no-snapshot", "-wipe-data"],
            env=self.env, stdout=self.output, stderr=subprocess.STDOUT, start_new_session=True)
        self.fd = os.pidfd_open(self.process.pid)
        parent, self.birth = identity(self.process.pid)
        require(parent == os.getpid(), "Guard is not our direct child")
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            require(self.process.poll() is None and identity(self.process.pid) == (os.getpid(), self.birth),
                    "Owned guard exited during emulator boot")
            rows = self.command([str(self.adb), "devices"], "boot-devices").splitlines()[1:]
            devices = [row.split() for row in rows if row.strip()]
            require(all(row[0] == SERIAL for row in devices), "Unexpected Android device during boot")
            if devices == [[SERIAL, "device"]]:
                self.owned()
                if self.run("shell", "getprop", "sys.boot_completed").strip() == "1":
                    self.run("shell", "input", "keyevent", "82")
                    return
            time.sleep(1)
        raise TimeoutError("Owned emulator did not boot within 180 seconds")

    def owned_process(self):
        require(self.process is not None and self.process.poll() is None, "Owned emulator guard is no longer live")
        require(identity(self.process.pid) == (os.getpid(), self.birth), "Guard birth changed")
        # The guard is a subreaper: require one exact live SDK image with the
        # unique AVD argv in its stable child ancestry before any adb mutation.
        found = []
        for entry in Path("/proc").iterdir():
            if not entry.name.isdigit():
                continue
            try:
                pid = int(entry.name)
                # The owned guard forwards these same -avd/-port arguments.
                # Its live birth was checked above; it is the ancestry root,
                # never a candidate SDK emulator process.
                if pid == self.process.pid:
                    continue
                before = identity(pid)
                words = (entry / "cmdline").read_bytes().split(b"\0")
                if b"-avd" not in words or self.name.encode() not in words:
                    continue
                if not any(words[i:i+2] == [b"-avd", self.name.encode()] for i in range(len(words)-1)):
                    continue
                if not any(words[i:i+2] == [b"-port", b"5582"] for i in range(len(words)-1)):
                    continue
                require((entry / "exe").resolve(strict=True).is_relative_to((self.sdk / "emulator").resolve()),
                        "AVD process is outside the SDK emulator")
                chain, current = [], pid
                for _ in range(128):
                    if current == self.process.pid:
                        break
                    info = identity(current)
                    chain.append((current, info))
                    current = info[0]
                    require(current > 1 and current not in [p for p, _ in chain], "Foreign emulator ancestry")
                require(current == self.process.pid and identity(pid) == before
                        and all(identity(p) == info for p, info in reversed(chain))
                        and identity(self.process.pid) == (os.getpid(), self.birth), "Unstable emulator ancestry")
                found.append(pid)
                observe_sdk_candidate(self, pid, before[1])  # sdk-candidate-witness:validated
            except (FileNotFoundError, ProcessLookupError):
                continue
        require(found, "No SDK emulator with the owned AVD/port and guard ancestry")

    def owned(self):
        self.owned_process()
        name = self.command([str(self.adb), "-s", SERIAL, "emu", "avd", "name"], "avd-name").splitlines()
        require(name == [self.name, "OK"], "ADB serial no longer names the owned AVD")
        self.owned_process()

    def run(self, *args, timeout=30, binary=False):
        self.owned()
        return self.command([str(self.adb), "-s", SERIAL, *map(str, args)],
                            "adb", timeout=timeout, binary=binary)

    def stop(self):
        observe_stop(self, "start")
        if self.process is None:
            observe_stop(self, "no_process")
            return None
        phase = "guard_poll"
        try:
            polled = self.process.poll()
            observe_stop(self, "guard_poll_returned", exit_code=polled)
            if polled is None:
                try:
                    phase = "emu_kill"
                    self.run("emu", "kill", timeout=15)
                    observe_stop(self, "emu_kill_returned")
                    phase = "guard_wait_15"
                    code = self.process.wait(timeout=15)
                    observe_stop(self, "guard_wait_15_returned", exit_code=code)
                except Exception as error:
                    observe_stop(self, "fallback", phase=phase, error=error)
                    # The retained pidfd cannot address a recycled/foreign PID.
                    phase = "pidfd_sigterm"
                    signal.pidfd_send_signal(self.fd, signal.SIGTERM)
                    observe_stop(self, "pidfd_sigterm_returned")
                    phase = "guard_wait_10"
                    code = self.process.wait(timeout=10)
                    observe_stop(self, "guard_wait_10_returned", exit_code=code)
            phase = "receipt_read"
            value = json.loads(self.proof.read_text())
            observe_stop(self, "receipt_read_returned")
            receipt(value, normal=False)
            observe_stop(self, "kernel_receipt_accepted")
            return value
        except Exception as error:
            observe_stop(self, "stop_error", phase=phase, error=error)
            raise
        finally:
            self.output.close()
            observe_stop(self, "output_closed")
            if self.fd is not None:
                os.close(self.fd)
                observe_stop(self, "pidfd_closed")
                self.fd = None


def preserve_guest_anr(emulator, scratch, out):
    """Failure-only reads; an absent/late diagnostic never qualifies the UI."""
    result = {"diagnosticOnly": True, "scope": "Exact owned disposable Android guest", "queries": {}}
    for label, query, limit in (
        ("guest-last-anr", ("dumpsys", "activity", "lastanr"), 128 * 1024),
        ("guest-last-anr-traces", ("dumpsys", "activity", "lastanr-traces"), 512 * 1024),
        # The newest activity trace may belong to a different boot service.
        # Retain timestamped ANR identities and only the system-app ANR tag.
        ("guest-anr-events", ("logcat", "-b", "events", "-d", "-v",
                              "threadtime,epoch,usec", "-s", "am_anr"), 128 * 1024),
        ("guest-system-app-anr", ("dumpsys", "dropbox", "--print", "system_app_anr"), 512 * 1024),
        # A blank screen need not be the ANR dialog: keep task/window state
        # and fatal Java/native records before retiring this exact guest.
        ("guest-activity-state", ("dumpsys", "activity", "activities"), 128 * 1024),
        ("guest-window-state", ("dumpsys", "window", "windows"), 128 * 1024),
        ("guest-crash", ("logcat", "-b", "crash", "-d", "-v", "threadtime,epoch,usec"), 128 * 1024),
    ):
        entry = result["queries"][label] = {}
        raw = scratch / (label + "-raw.log")
        try:
            require(not raw.exists() and not raw.is_symlink(), "Existing diagnostic input refused")
            emulator.owned()
        except Exception as error:
            entry["errorType"] = type(error).__name__
            break  # A foreign/dead guest authorizes no further reads.
        try:
            runtime.command([str(emulator.adb), "-s", SERIAL, "shell", *query],
                            raw, timeout=10, read_output=False)
            entry["queryReturned"] = True
        except Exception as error:
            entry["errorType"] = type(error).__name__
        try:
            if raw.exists():
                require(raw.is_file() and not raw.is_symlink(), "Plain diagnostic output required")
                size = raw.stat().st_size
                with raw.open("rb") as source:
                    data = source.read(limit)
                with (out / (label + ".log")).open("xb") as target:
                    target.write(data)
                entry.update(sourceBytes=size, keptBytes=len(data), truncated=size > len(data),
                             sha256=hashlib.sha256(data).hexdigest())
                if label == "guest-activity-state":
                    # Reuse this already-read snapshot; no new PID/process query.
                    entry["appProcessPid"] = (native_activity_pid(data, truncated=size > len(data))
                                              if entry.get("queryReturned") else None)
        except Exception as error:
            entry["collectionErrorType"] = type(error).__name__
    return result


# native-observation-begin:collector
NATIVE_OBSERVATION_TAG = "TakosWebHomeNative"
NATIVE_OBSERVATION_STAGES = (
    "BACK_ENTER", "BACK_RETURN", "BIND_ENTER", "ADD_RETURN", "BIND_RETURN",
    "UNBIND_ENTER", "REMOVE_RETURN", "UNBIND_RETURN", "RESUME", "PAUSE",
    "PAGE_START", "PAGE_COMMIT", "PAGE_FINISH", "RENDER_GONE", "CAP_EXHAUSTED",
)
NATIVE_OBSERVATION_QUERY = ("logcat", "-b", "main", "-d", "-v", "raw", "-t", "64")
NATIVE_OBSERVATION_READ_BYTES = 64 * 1024
NATIVE_OBSERVATION_SAVE_BYTES = 32 * 1024
NATIVE_OBSERVATION_PACKET = re.compile(
    r"v=1 seq=([0-9]{1,2}) pid=([0-9]{1,10}) t=([0-9]{1,19}) stage=("
    + "|".join(NATIVE_OBSERVATION_STAGES)
    + r") view=(-?[0-9]{1,10}) cur=([01]) host=([01]) gen=(-?[0-9]{1,19})"
      r" vis=([01]) res=([01]) evict=([01]) logical=(UNKNOWN|ADD_RETURN|REMOVE_RETURN)"
      r" physical=unknown cap=([01])"
)


def native_activity_pid(data, *, truncated=False):
    """A scalar from one complete existing Activity snapshot, never PID authority."""
    if truncated or len(data) > 128 * 1024:
        return None
    try:
        lines = data.decode("ascii").splitlines()
    except UnicodeDecodeError:
        return None
    component = PACKAGE + "/com.takosumi.appscreen.AppScreenActivity"
    record = r"ActivityRecord\{([0-9a-f]+) u([0-9]+) " + re.escape(component) + r" t([0-9]+)\}"
    top = [line.strip() for line in lines if "topResumedActivity=" in line]
    if len(top) != 1:
        return None
    resumed = re.fullmatch(r"topResumedActivity=" + record, top[0])
    if not resumed:
        return None
    histories = [(index, re.fullmatch(r"([ ]*)\* Hist +#[0-9]+: " + record, line))
                 for index, line in enumerate(lines)]
    matching = [(index, match) for index, match in histories
                if match and match.groups()[1:] == resumed.groups()]
    if len(matching) != 1:
        return None
    index, header = matching[0]
    indentation = len(header[1])
    block = []
    for line in lines[index + 1:]:
        if line.strip() and len(line) - len(line.lstrip(" ")) <= indentation:
            break  # A complete Hist block must end before this snapshot ends.
        block.append(line.strip())
    else:
        return None
    fields = {key: [line for line in block if line.startswith(key + "=")]
              for key in ("packageName", "app", "mActivityComponent")}
    if (fields["packageName"] != ["packageName=" + PACKAGE + " processName=" + PACKAGE]
            or fields["mActivityComponent"] != ["mActivityComponent=" + component]
            or len(fields["app"]) != 1):
        return None
    process = re.fullmatch(r"app=ProcessRecord\{[0-9a-f]+ ([1-9][0-9]{0,9}):"
                           + re.escape(PACKAGE) + r"/u([0-9]+)a[0-9]+\}", fields["app"][0])
    if not process or process[2] != resumed[2]:
        return None
    pid = int(process[1])
    return pid if pid <= 2**31 - 1 else None


def parse_native_observation(data):
    """Only the closed primitive grammar survives; raw log/user strings do not."""
    records, rejected, cut = [], 0, False
    for line in data.splitlines():
        match = NATIVE_OBSERVATION_PACKET.fullmatch(line.decode("ascii", "replace")) if len(line) <= 320 else None
        if not match:
            rejected += 1
            continue
        seq, pid, elapsed, stage, view, current, host, generation, visible, resumed, evicted, logical, cap = match.groups()
        seq, pid, elapsed, view, generation = map(int, (seq, pid, elapsed, view, generation))
        if not (1 <= seq <= 64 and 1 <= pid <= 2**31 - 1 and 0 <= elapsed <= 2**63 - 1
                and -(2**31) <= view <= 2**31 - 1 and -(2**63) <= generation <= 2**63 - 1
                and (cap == "1") == (stage == "CAP_EXHAUSTED") == (seq == 64)):
            rejected += 1
            continue
        if len(records) >= 64:
            cut = True
            break
        records.append({"seq": seq, "pid": pid, "elapsedRealtimeMs": elapsed,
                        "stage": stage, "viewIdentity": view, "currentEntry": current == "1",
                        "host": host == "1", "generation": generation, "visible": visible == "1",
                        "resumed": resumed == "1", "evicted": evicted == "1",
                        "logical": logical, "physical": "unknown", "capExhausted": cap == "1"})
    return records, rejected, cut


def preserve_native_observation(emulator, scratch, out, prior):
    """One failure-only closed-tag tail; no absolute aggregate/coverage claim."""
    result = {"diagnosticOnly": True, "clockDomain": "android_elapsed_realtime", "clockUnit": "ms",
              "captureWindow": "pid_main_tail_then_closed_tag_filter", "coverage": "unknown",
              "queryReturned": False, "records": [], "commandWaitSeconds": 10,
              "packetTailCount": 64, "rawReadCapBytes": NATIVE_OBSERVATION_READ_BYTES,
              "saveCapBytes": NATIVE_OBSERVATION_SAVE_BYTES}
    required = {"guest-last-anr", "guest-last-anr-traces", "guest-anr-events", "guest-system-app-anr",
                "guest-activity-state", "guest-window-state", "guest-crash"}
    previous = prior.get("queries", {}) if isinstance(prior, dict) else {}
    if set(previous) != required or any(not isinstance(entry, dict) or not (entry.get("queryReturned") or "sourceBytes" in entry)
                                       for entry in previous.values()):
        result["skipped"] = "prior_diagnostic_incomplete"
        return result  # Do not recheck a guest after the existing collector lost custody.
    pid = previous["guest-activity-state"].get("appProcessPid")
    if type(pid) is not int or not 1 <= pid <= 2**31 - 1:
        result["skipped"] = "app_process_unknown"
        return result  # No broad tail, lookup or retry without the existing witness.
    result["appProcessPid"] = pid
    raw = scratch / "guest-native-observation-raw.log"
    try:
        require(not raw.exists() and not raw.is_symlink(), "Existing diagnostic input refused")
        emulator.owned()
    except BaseException as error:
        result["error"] = "interrupted" if isinstance(error, (KeyboardInterrupt, SystemExit)) else "ownership_or_input_refused"
        return result
    try:
        runtime.command([str(emulator.adb), "-s", SERIAL, "shell", *NATIVE_OBSERVATION_QUERY,
                         "--pid=" + str(pid), "-s", NATIVE_OBSERVATION_TAG + ":I", "*:S"],
                        raw, timeout=10, read_output=False)
        result["queryReturned"] = True
    except BaseException as error:
        result["error"] = ("interrupted" if isinstance(error, (KeyboardInterrupt, SystemExit)) else
                           "timeout" if isinstance(error, (TimeoutError, subprocess.TimeoutExpired)) else "query_failed")
    try:
        if raw.exists():
            with os.fdopen(os.open(raw, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as source:
                info = os.fstat(source.fileno())
                require(stat.S_ISREG(info.st_mode), "Plain diagnostic output required")
                data = source.read(NATIVE_OBSERVATION_READ_BYTES + 1)
            prefix = data[:NATIVE_OBSERVATION_READ_BYTES]
            # A cut trailing packet is unknown, never parsed as a whole record.
            truncated = info.st_size > len(prefix) or len(data) > len(prefix)
            if truncated and not prefix.endswith(b"\n"):
                prefix = prefix.rsplit(b"\n", 1)[0] if b"\n" in prefix else b""
            records, rejected, cut = parse_native_observation(prefix)
            mismatched = sum(record["pid"] != pid for record in records)
            records = [record for record in records if record["pid"] == pid]
            result.update(records=records, pidMismatchPackets=mismatched,
                          sourceBytes=info.st_size, rawPrefixBytes=len(prefix),
                          rawPrefixTruncated=truncated, rejectedPackets=rejected, recordLimitReached=cut,
                          nativeCapSeen=any(record["capExhausted"] for record in records))
        result["savedRecordsTruncated"] = False
        payload = json.dumps(result, sort_keys=True, separators=(",", ":")).encode("ascii")
        while len(payload) > NATIVE_OBSERVATION_SAVE_BYTES and result["records"]:
            result["records"].pop()
            result["savedRecordsTruncated"] = True
            payload = json.dumps(result, sort_keys=True, separators=(",", ":")).encode("ascii")
        require(len(payload) <= NATIVE_OBSERVATION_SAVE_BYTES, "Diagnostic output cap")
        fd = os.open(out / "guest-native-observation.json", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "wb") as target:
            target.write(payload)
    except BaseException as error:
        result["collectionError"] = "interrupted" if isinstance(error, (KeyboardInterrupt, SystemExit)) else "output_refused"
    return result
# native-observation-end:collector


def installed(emulator, expected):
    paths = emulator.run("shell", "pm", "path", PACKAGE).strip().splitlines()
    require(len(paths) == 1 and paths[0].startswith("package:"), "One installed base APK required")
    path = paths[0].removeprefix("package:")
    require(re.fullmatch(r"/data/app/[A-Za-z0-9_./=+~-]+/base\.apk", path), "Foreign installed APK path")
    reply = emulator.run("shell", "sha256sum", path).strip().split()
    require(len(reply) == 2 and reply == [expected, path], "Installed APK differs from the verified build")
    return expected


def driver_apks(paths, sdk, command):
    require(len(paths) == 2, "Exactly one driver and instrumentation APK are required")
    identities, proof = set(), []
    android = "{http://schemas.android.com/apk/res/android}"
    for path in paths:
        require(path.is_file() and not path.is_symlink(), "Plain driver APK required")
        manifest = ET.fromstring(command([str(sdk / "cmdline-tools/latest/bin/apkanalyzer"),
                                         "manifest", "print", str(path)], "driver-manifest"))
        package = manifest.get("package")
        require(package in (DRIVER, DRIVER + ".test") and package not in identities,
                "Unexpected or duplicate driver APK identity")
        identities.add(package)
        require(not any(action.get(android + "name") == "android.intent.action.MAIN"
                        for action in manifest.findall("application/activity/intent-filter/action")),
                "The external driver must not supply a substitute App launcher")
        instrumentations = manifest.findall("instrumentation")
        if package.endswith(".test"):
            require(len(instrumentations) == 1
                    and instrumentations[0].get(android + "targetPackage") == DRIVER
                    and instrumentations[0].get(android + "name") == "androidx.test.runner.AndroidJUnitRunner",
                    "Instrumentation must run in its separate driver package")
        else:
            require(not instrumentations, "Unexpected instrumentation in driver host")
        command([str(sdk / "build-tools/36.0.0/apksigner"), "verify", str(path)], "driver-signature")
        proof.append({"package": package, "name": path.name, "sha256": digest(path)})
    return proof


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    require(platform.system() == "Linux" and platform.machine() == "x86_64", "Linux x86_64 SDK host required")
    require(not any(key in os.environ for key in (*runtime.SIGNING, "ANDROID_KEYSTORE", "ANDROID_KEYSTORE_PATH",
            "ANDROID_KEYSTORE_PASSWORD", "ANDROID_KEY_ALIAS", "ANDROID_KEY_PASSWORD",
            "TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD")), "Signing inputs refused")
    sdk = Path(os.environ["ANDROID_HOME"]).resolve(strict=True)
    out = args.out.resolve()
    require(not out.is_relative_to(ROOT), "Evidence must stay outside the repository")
    out.mkdir(mode=0o700)
    baseline = verify_app(ROOT)
    apk = ROOT / baseline["apk"]["path"]
    parent = Path(os.environ.get("RUNNER_TEMP", "/root/dev/tmp")).resolve(strict=True)
    require(not parent.is_relative_to(ROOT) and shutil.disk_usage(parent).free >= 8 * 1024**3,
            "Need 8 GiB private scratch space outside source")
    scratch = Path(tempfile.mkdtemp(prefix="sumi-android-web-home-", dir=parent))
    scratch.chmod(0o700)
    sequence = 0
    def command(argv, label, timeout=60, env=None, binary=False):
        nonlocal sequence
        sequence += 1
        path = out / f"{sequence:04d}-{label}.log"
        value = runtime.command(argv, path, timeout=timeout, env=env)
        return path.read_bytes() if binary else value.replace("\r\n", "\n")
    proof = {"passed": False, "functionalPassed": False, "scope": "Actual installed Tauri Android debug App; synthetic Web UI",
             "normalSDKApp": baseline, "cleanup": False, "normalAPKRestored": False, "scratchRemoved": False,
             "adbServiceScope": "Existing adb service is outside the emulator guard; never killed"}
    servers, overlay, emulator, variant, failure = [], None, None, None, None
    backup = scratch / "normal.apk"
    shutil.copyfile(apk, backup)
    backup_custody = FileCustody(backup, scratch)
    variant_custody = None
    try:
        ca, cert, key = runtime.certificate(scratch, localhost=True)
        overlay = Overlay(ROOT, ca)
        overlay.apply()
        command(["bunx", "tauri", "android", "build", "--ci", "--debug", "--target", "x86_64", "--apk",
                 "--config", '{"identifier":"com.takosumi.fixture.android"}', "--", "--locked"],
                "fixture-app-build", timeout=600)
        variant = verify_app(ROOT, fixture_ca=ca)
        variant_custody = FileCustody(apk, ROOT / "src-tauri/gen/android")
        require(variant["cargoAppSha256"] == baseline["cargoAppSha256"], "Fixture rebuild changed the Cargo App library")
        proof["uiApp"] = variant
        instance, editor = Site(cert, key, "instance"), Site(cert, key, "editor")
        servers = [instance, editor]
        instance.editor = editor
        for server in servers:
            server.start()
        driver = HERE / "web-home-fixture"
        command(["gradle", "--no-daemon", "-p", str(driver), "assembleDebug", "assembleDebugAndroidTest"],
                "driver-build", timeout=180)
        apks = list((driver / "build/outputs/apk").rglob("*.apk"))
        proof["driverAPKs"] = driver_apks(sorted(apks), sdk, command)
        emulator = Emulator(sdk, scratch, out, command)
        emulator.start()
        for server in servers:
            port = f"tcp:{server.server_port}"
            emulator.run("reverse", port, port)
        emulator.run("install", "-t", apk, timeout=120)
        proof["installedBefore"] = installed(emulator, variant["apk"]["sha256"])
        for path in sorted(apks):
            emulator.run("install", "-t", path, timeout=60)
        try:
            log = emulator.run("shell", "am", "instrument", "-w", "-r", "-e", "class", CLASS + "#" + METHOD,
                "-e", "instance", instance.url, "-e", "editor", editor.url,
                "-e", "launcher", variant["launcher"], DRIVER + ".test/androidx.test.runner.AndroidJUnitRunner", timeout=600)
        finally:
            # A failed case must retain its actual UI evidence too. Optional
            # diagnostics never replace the instrumentation's original error.
            for name in ("result.json", "screen.png", "hierarchy.xml"):
                try:
                    data = emulator.run("exec-out", "run-as", DRIVER, "cat", "files/" + name, binary=True)
                    (out / name).write_bytes(data)
                except Exception as error:
                    proof.setdefault("diagnosticErrors", []).append(name + ":" + type(error).__name__)
        result = json.loads((out / "result.json").read_text())
        proof["test"] = require_ui_pass(log, result)
        require_site_events(instance.events, editor.events)
        proof["installedAfter"] = installed(emulator, variant["apk"]["sha256"])
        require(verify_app(ROOT, fixture_ca=ca) == variant, "App build changed during UI operations")
        proof["functionalPassed"] = True
    except BaseException as error:
        failure = error
        proof["errorType"] = type(error).__name__
        proof["error"] = str(error)
        if isinstance(error, Exception) and emulator is not None:
            proof["guestANR"] = preserve_guest_anr(emulator, scratch, out)
            # native-observation-begin:failure-call
            try:
                proof["nativeObservation"] = preserve_native_observation(emulator, scratch, out, proof["guestANR"])
            except BaseException:
                proof["nativeObservation"] = {"diagnosticOnly": True, "error": "collector_failed", "coverage": "unknown"}
            # native-observation-end:failure-call
    finally:
        cleanup_errors = []
        if emulator is not None:
            try:
                proof["emulatorGuard"] = emulator.stop()
                receipt(proof["emulatorGuard"], normal=proof["functionalPassed"])
                proof["cleanup"] = True
            except Exception as error:
                cleanup_errors.append("emulator:" + type(error).__name__)
            finally:
                preserve_stop_observation(emulator, proof)
                preserve_sdk_candidate_witness(emulator, proof)  # sdk-candidate-witness:summary
        for server in servers:
            try:
                server.stop()
            except Exception as error:
                cleanup_errors.append("site:" + type(error).__name__)
        if servers:
            (out / "site-events.json").write_text(json.dumps([s.events for s in servers], indent=2) + "\n")
        # Reaping a timed-out CLI does not prove its Gradle/Tauri descendants
        # are idle. Keep the private project/CA/APK backup intact in that case.
        if not runtime.INTERRUPTED_COMMANDS and not cleanup_errors and (emulator is None or proof["cleanup"]):
            try:
                require(overlay is not None and variant is not None, "Incomplete overlay/build; preserve private inputs")
                require(variant_custody is not None, "Missing App artifact custody")
                require(variant_custody.observe() == variant_custody.before
                        and backup_custody.observe() == backup_custody.before, "App artifact custody changed")
                overlay.restore()
                restore_apk(variant_custody, backup_custody, variant["apk"]["sha256"], baseline["apk"]["sha256"])
                require(verify_app(ROOT) == baseline, "Normal SDK App was not exactly restored")
                proof["normalAPKRestored"] = True
                shutil.rmtree(scratch)
                proof["scratchRemoved"] = True
            except Exception as error:
                cleanup_errors.append("restore:" + type(error).__name__)
        proof["interruptedCLI"] = bool(runtime.INTERRUPTED_COMMANDS)
        proof["cleanupErrors"] = cleanup_errors
        proof["passed"] = (proof["functionalPassed"] and proof["cleanup"] and proof["normalAPKRestored"]
                           and proof["scratchRemoved"] and not proof["interruptedCLI"] and not cleanup_errors)
        (out / "summary.json").write_text(json.dumps(proof, indent=2) + "\n")
    if failure is not None:
        raise failure
    require(proof["passed"], "Whole Android App runtime qualification incomplete; see summary.json")


if __name__ == "__main__":
    main()
