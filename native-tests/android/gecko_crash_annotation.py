"""Private, bounded Gecko fixture annotation projection; never a crash verdict.

Raw .extra bytes live only in the reader's memory. Native minidumps are never
read. Package/report custody does not establish the crashed PID or lifetime.
"""
import json
import os
from pathlib import Path
import re
import selectors
import subprocess
import sys
import time

PACKAGE = "com.takosumi.fixture.geckodownload"
PACKAGES = (PACKAGE + ".test", PACKAGE)
SERIAL = "emulator-5562"
BUILD = "20261102144711"
FILES = 4
FILE_BYTES = 65536
TOTAL_BYTES = FILES * FILE_BYTES
OUTPUT_BYTES = 16384
CAPTURE_SECONDS = 12
READ_SECONDS = 3
FRAME_BYTES = 128
NAME = re.compile(r"files/minidumps/[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}\.extra\Z")
THROWABLES = frozenset((
    "java.lang.IllegalStateException", "java.lang.IllegalArgumentException",
    "java.lang.NullPointerException", "java.lang.RuntimeException",
    "java.lang.SecurityException", "java.lang.OutOfMemoryError",
    "java.lang.NoSuchMethodError", "java.lang.ExceptionInInitializerError",
    "java.io.IOException", "android.os.DeadObjectException", "android.os.RemoteException",
))
FRAMES = {
    "org.mozilla.gecko.process.GeckoProcessManager.start": ("GeckoProcessManager.java", "process-start"),
    "org.mozilla.gecko.process.GeckoProcessManager.startInternal": ("GeckoProcessManager.java", "process-start-internal"),
    "android.app.ContextImpl.bindServiceCommon": ("ContextImpl.java", "bind-service"),
    "android.app.ContextImpl.bindService": ("ContextImpl.java", "bind-service"),
    "android.app.ContextImpl.startServiceCommon": ("ContextImpl.java", "start-service"),
    "android.os.Parcel.readException": ("Parcel.java", "read-exception"),
}


class Refused(Exception):
    """Only fixed internal status codes may escape to the projection."""


def bounded(command, cap, deadline):
    """Drain at most cap+1 bytes and retire only our own command on refusal."""
    if time.monotonic() >= deadline:
        raise Refused("timeout")
    child = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                             stderr=subprocess.DEVNULL)
    data = bytearray()
    until = min(deadline, time.monotonic() + READ_SECONDS)
    try:
        with selectors.DefaultSelector() as poll:
            poll.register(child.stdout, selectors.EVENT_READ)
            while True:
                remaining = until - time.monotonic()
                if remaining <= 0:
                    raise Refused("timeout")
                if not poll.select(remaining):
                    raise Refused("timeout")
                part = os.read(child.stdout.fileno(), min(8192, cap + 1 - len(data)))
                if not part:
                    break
                data.extend(part)
                if len(data) > cap:
                    raise Refused("overcap")
        if child.wait(timeout=max(0.001, until - time.monotonic())) != 0:
            raise Refused("command-refused")
        return bytes(data)
    except subprocess.TimeoutExpired:
        raise Refused("timeout") from None
    finally:
        if child.poll() is None:
            child.kill()
        child.wait(timeout=1)
        child.stdout.close()


def adb(args, cap, deadline, run=bounded):
    return run(["adb", "-s", SERIAL, *args], cap, deadline)


def package_uids(deadline, run=bounded):
    raw = adb(["shell", "cmd", "package", "list", "packages", "-U", "--user", "0", PACKAGE],
              4096, deadline, run).decode("ascii")
    result = {}
    for line in raw.splitlines():
        match = re.fullmatch(r"package:([a-zA-Z0-9_.]+) uid:([0-9]{5,8})", line)
        if not match or match[1] not in PACKAGES or match[1] in result:
            raise Refused("package-identity")
        uid = int(match[2])
        if not 10000 <= uid < 100000:
            raise Refused("package-identity")
        result[match[1]] = uid
    return result


def _decode_frame(raw, kind, limit):
    """exec-out host0 says nothing about the remote command's final checks."""
    header, separator, rest = raw.partition(b"\n")
    match = re.fullmatch(rb"SGECKO/1 (OWNER|LIST|FILE) ([0-4]) (0|[1-9][0-9]{0,5})", header)
    if not separator or not match or match[1].decode() != kind:
        raise Refused("transport-framing")
    count, size = int(match[2]), int(match[3])
    if size > limit:
        raise Refused("overcap")
    trailer = f"\nSGECKO/1 END {kind} {count} {size}\n".encode()
    if len(rest) != size + len(trailer) or rest[size:] != trailer:
        raise Refused("transport-framing")
    return rest[:size], count


def _shell_frame(kind, count):
    # Body is a bounded metadata variable, never the raw .extra file.
    return ("size=$(( $(printf %s \"$body\" | wc -c) )); "
            f"printf 'SGECKO/1 {kind} %s %s\\n' {count} \"$size\"; "
            "printf %s \"$body\"; "
            f"printf '\\nSGECKO/1 END {kind} %s %s\\n' {count} \"$size\"")


def owner(uid, deadline, run=bounded, package=PACKAGE):
    if package not in PACKAGES:
        raise Refused("package-identity")
    script = ("uid=$(id -u) && here=$(pwd -P) || exit 1; "
              "body=$(printf '%s\\n%s' \"$uid\" \"$here\"); body=\"$body\n\"; " +
              _shell_frame("OWNER", "2"))
    wire = adb(["exec-out", "run-as", package, "/system/bin/sh", "-c", script],
               256 + FRAME_BYTES, deadline, run)
    data, count = _decode_frame(wire, "OWNER", 256)
    if count != 2:
        raise Refused("transport-framing")
    raw = data.decode("ascii").splitlines()
    if raw != [str(uid), "/data/user/0/" + package]:
        raise Refused("run-as-identity")


def report_names(deadline, run=bounded):
    # Stop before an excessive listing and refuse symlinked directory components.
    script = ("test ! -L files && test ! -L files/minidumps && test -d files/minidumps || exit 1; "
              "n=0; body=''; for p in files/minidumps/*.extra; do "
              "test -e \"$p\" || test -L \"$p\" || continue; "
              "n=$((n+1)); test \"$n\" -le 4 || exit 1; body=\"$body$p\n\"; done; " +
              _shell_frame("LIST", '"$n"'))
    wire = adb(["exec-out", "run-as", PACKAGE, "/system/bin/sh", "-c", script],
               512 + FRAME_BYTES, deadline, run)
    data, count = _decode_frame(wire, "LIST", 512)
    raw = data.decode("ascii").splitlines()
    if len(raw) != count or len(raw) > FILES or len(raw) != len(set(raw)) or any(not NAME.fullmatch(s) for s in raw):
        raise Refused("report-path")
    return raw


def read_report(name, deadline, run=bounded):
    if not NAME.fullmatch(name):
        raise Refused("report-path")
    # Validate the opened FD before reading it, not just the pathname before open.
    # All interpolated characters are from the strict UUID/path grammar above.
    script = (
        "test ! -L files && test ! -L files/minidumps && "
        f"test ! -L {name} && test -f {name} || exit 1; "
        f"test \"$(stat -c %h {name})\" = 1 || exit 1; "
        f"test \"$(stat -c %s {name})\" -le 65536 || exit 1; "
        f"before=$(stat -c '%d:%i:%s:%h:%Y:%Z' {name}) || exit 1; "
        f"exec 3< {name}; opened=$(stat -L -c '%d:%i:%s:%h:%Y:%Z' /proc/$$/fd/3) || exit 1; "
        f"test \"$(readlink /proc/$$/fd/3)\" = /data/user/0/{PACKAGE}/{name} || exit 1; "
        "test \"$before\" = \"$opened\" || exit 1; "
        "size=$(stat -L -c %s /proc/$$/fd/3) || exit 1; "
        "test \"$size\" -ge 0 && test \"$size\" -le 65536 || exit 1; "
        "printf 'SGECKO/1 FILE 1 %s\\n' \"$size\"; "
        "head -c \"$size\" <&3 || exit 1; "
        "after=$(stat -L -c '%d:%i:%s:%h:%Y:%Z' /proc/$$/fd/3) || exit 1; "
        f"test ! -L files && test ! -L files/minidumps && test ! -L {name} || exit 1; "
        f"current=$(stat -c '%d:%i:%s:%h:%Y:%Z' {name}) || exit 1; "
        "test \"$opened\" = \"$after\" && test \"$after\" = \"$current\" || exit 1; "
        "printf '\\nSGECKO/1 END FILE 1 %s\\n' \"$size\""
    )
    wire = adb(["exec-out", "run-as", PACKAGE, "/system/bin/sh", "-c", script],
               FILE_BYTES + FRAME_BYTES, deadline, run)
    data, count = _decode_frame(wire, "FILE", FILE_BYTES)
    if count != 1:
        raise Refused("transport-framing")
    return data


def unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise Refused("invalid-report")
        result[key] = value
    return result


def project(raw):
    if len(raw) > FILE_BYTES:
        raise Refused("overcap")
    value = json.loads(raw.decode("utf-8"), object_pairs_hook=unique)
    if not isinstance(value, dict) or len(value) > 256:
        raise Refused("invalid-report")
    if value.get("BuildID") != BUILD:
        raise Refused("report-identity")
    trace = value.get("JavaStackTrace")
    if not isinstance(trace, str) or len(trace) > 32768:
        raise Refused("annotation-unavailable")
    lines = trace.splitlines()
    if not lines or len(lines) > 256 or any(len(s) > 2048 for s in lines):
        raise Refused("annotation-unavailable")
    first = re.match(r"([a-zA-Z0-9_.$]+)(?::|$)", lines[0])
    throwable = first[1] if first and first[1] in THROWABLES else "unknown"
    frames = []
    for line in lines[1:]:
        match = re.fullmatch(r"\s+at ([a-zA-Z0-9_.$]+)\(([a-zA-Z0-9_.$]+):([1-9][0-9]{0,5})\)", line)
        if match and match[1] in FRAMES and match[2] == FRAMES[match[1]][0]:
            if len(frames) == 8:
                raise Refused("overcap")
            frames.append({"kind": FRAMES[match[1]][1], "line": int(match[3])})
    return {"status": "projected", "sameCrash": "unknown", "throwableClass": throwable,
            "frames": frames, "buildMatches": value.get("BuildID") == BUILD}


def collect(run=bounded):
    deadline = time.monotonic() + CAPTURE_SECONDS
    stage = "package-before"
    try:
        uids = package_uids(deadline, run)
        if PACKAGE not in uids:
            raise Refused("package-unavailable")
        stage = "owner-before"
        owner(uids[PACKAGE], deadline, run)
        stage = "list"
        names = report_names(deadline, run)
        records, total = [], 0
        for name in names:
            stage = "file"
            raw = read_report(name, deadline, run)
            total += len(raw)
            if total > TOTAL_BYTES:
                raise Refused("overcap")
            records.append(project(raw))
        stage = "owner-after"
        owner(uids[PACKAGE], deadline, run)
        stage = "package-after"
        if package_uids(deadline, run) != uids:
            raise Refused("package-identity")
        return {"status": "projected" if records else "empty", "sameCrash": "unknown",
                "records": records, "rawBytesRead": total}
    except Refused as error:
        allowed = {"timeout", "overcap", "command-refused", "package-identity", "run-as-identity",
                   "report-path", "invalid-report", "annotation-unavailable", "package-unavailable"}
        allowed.update(("report-identity", "transport-framing"))
        return {"status": "unknown", "reason": str(error) if str(error) in allowed else "reader-refused",
                "stage": stage}
    except Exception:
        # Never print arbitrary decoder/ADB errors or raw annotation contents.
        return {"status": "unknown", "reason": "reader-refused", "stage": stage}


def retire(run=bounded):
    result = []
    for package in PACKAGES:
        deadline = time.monotonic() + 5
        try:
            before = package_uids(deadline, run)
            if package not in before:
                result.append({"package": package, "status": "absent"})
                continue
            owner(before[package], deadline, run, package)
            raw = adb(["uninstall", package], 256, deadline, run)
            if raw.strip() != b"Success" or package in package_uids(deadline, run):
                raise Refused("retirement")
            result.append({"package": package, "status": "uninstalled"})
        except Exception:
            result.append({"package": package, "status": "failed"})
    return result


def finish(step, output, run=bounded):
    """Optional diagnostics cannot erase the original exit or skip retirement."""
    projection = {"status": "not-requested"}
    try:
        if step:
            projection = collect(run)
    except Exception:
        projection = {"status": "unknown", "reason": "reader-refused"}
    finally:
        retirement = retire(run)
    result = step or int(any(r["status"] == "failed" for r in retirement))
    proof = {"diagnosticOnly": True, "originalStepExit": step, "annotation": projection,
             "retirement": retirement, "rawSaved": False, "minidumpRead": False}
    try:
        data = (json.dumps(proof, separators=(",", ":"), ensure_ascii=True) + "\n").encode()
        if len(data) > OUTPUT_BYTES:
            raise Refused("overcap")
        fd = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
    except Exception:
        result = step or 1
    return result


def main():
    if len(sys.argv) != 3 or not sys.argv[1].isdigit() or not 0 <= int(sys.argv[1]) <= 255:
        return 2
    return finish(int(sys.argv[1]), Path(sys.argv[2]))


if __name__ == "__main__":
    raise SystemExit(main())
