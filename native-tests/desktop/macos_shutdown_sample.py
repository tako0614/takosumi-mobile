"""Fixture-only, bounded stack evidence for an owned Mac browser in CefShutdown.

Sampling may perturb scheduling. It never changes the runner's timeouts or
receipt verdict and is not an unsampled qualification run. No process-name
search, signals, heap dump, environment or page data is requested.
"""
from contextlib import contextmanager
import json
import os
from pathlib import Path
import select
import signal
import subprocess
import sys
import tempfile
import time

LIMIT = 4 * 1024 * 1024
STDERR_LIMIT = 8192
PREFIX = "takosumi-cef-mac-lifecycle: "
DIAGNOSTIC_BUDGET = 45.0
RETIRE_BUDGET = 2.0
REPLY_LIMIT = 16384
_active_diagnostic = None


class DiagnosticRetirementError(RuntimeError):
    def __init__(self, receipt):
        super().__init__("The spawned diagnostic helper did not retire")
        self.receipt = receipt


def shutdown_pending(path, identity):
    """Only the current PID/birth's bounded lifecycle tail; old launches do not count."""
    with path.open("rb") as stream:
        stream.seek(0, 2)
        stream.seek(max(0, stream.tell() - 16384))
        lines = stream.read(16384).decode("utf-8", errors="replace").splitlines()
    pending = False
    for line in lines:
        if not line.startswith(PREFIX):
            continue
        try:
            event = json.loads(line[len(PREFIX):])
        except ValueError:
            continue
        if not isinstance(event, dict):
            continue
        if event.get("pid") != identity["pid"] or event.get("start") != identity["start"]:
            continue
        if event.get("event") == "cef-shutdown-begin":
            pending = True
        elif event.get("event") == "cef-shutdown-return":
            pending = False
    return pending


def same_owner(processes, child, parent, browser, guard):
    return (child["parent"] == parent["pid"] and child["start"] >= parent["start"]
            and processes.facts(child["pid"]) == child
            and processes.facts(parent["pid"]) == parent
            and browser.matches(processes.image(child["pid"]))
            and guard.matches(processes.image(parent["pid"]))
            and processes.facts(child["pid"]) == child
            and processes.facts(parent["pid"]) == parent)


def retain_bounded(source, output, limit):
    if not source.is_file() or source.is_symlink():
        return None
    with source.open("rb") as stream:
        data = stream.read(limit + 1)
    with output.open("xb") as stream:
        stream.write(data[:limit])
    return {"bytes": min(len(data), limit), "truncated": len(data) > limit}


def capture_state(child, output):
    """Kernel-side state for the owned PID: process row, thread rows, open FDs.

    Unlike sample these reads never wait on the target: when the wedged
    browser cannot be symbolicated at all, they still name the subsystem it
    was holding (thread states, its live sockets/files). Best effort only;
    each probe records its own exit and changes no verdict.
    """
    record = {}
    chunks = []
    for name, argv, bound in (
        ("ps", ["/bin/ps", "-o", "pid,ppid,pgid,stat,etime,time,comm", "-p", str(child["pid"])], 10),
        ("threads", ["/bin/ps", "-M", "-p", str(child["pid"])], 10),
        ("files", ["/usr/sbin/lsof", "-nP", "-p", str(child["pid"])], 15),
    ):
        try:
            completed = subprocess.run(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                     stderr=subprocess.STDOUT, timeout=bound, check=False)
            record[name] = {"exit": completed.returncode}
            chunks.append(b"$ " + " ".join(argv).encode() + b"\n" + completed.stdout[:262144])
        except subprocess.TimeoutExpired:
            record[name] = {"errorType": "TimeoutExpired"}
        except Exception as error:
            record[name] = {"errorType": type(error).__name__}
    try:
        with output.open("xb") as stream:
            stream.write(b"\n".join(chunks)[:LIMIT])
        record["stateBytes"] = output.stat().st_size
    except OSError as error:
        record["stateWriteError"] = type(error).__name__
    return record


def _sample_owned(processes, child, parent, browser, guard, output):
    """One one-second sample; discard output if process identity changed meanwhile."""
    result = {"pid": child["pid"], "start": child["start"], "diagnosticOnly": True}
    try:
        if not same_owner(processes, child, parent, browser, guard):
            return dict(result, status="identity-changed-before")
        output.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        result["state"] = capture_state(child, output.with_suffix(".state.log"))
        # sample owns only its new temporary output. A timeout kills/reaps
        # that subprocess through subprocess.run, never the sampled browser.
        with tempfile.TemporaryDirectory(prefix="sample-", dir=output.parent) as scratch:
            raw = Path(scratch) / "stack.log"
            diagnostic = Path(scratch) / "stderr.log"
            timed_out = False
            with diagnostic.open("wb") as stderr:
                try:
                    # -mayDie grabs symbol locations first: the teardown watchdog
                    # can kill the sampled browser mid-run. Three- and five-
                    # second attempts expired before producing a report. This
                    # seven-second command allowance excludes the preceding
                    # state reads, ownership checks and filesystem work; it
                    # cannot guarantee completion before the teardown watchdog.
                    completed = subprocess.run(["/usr/bin/sample", str(child["pid"]), "1", "100",
                                                "-mayDie", "-file", str(raw)],
                                               stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                               stderr=stderr, timeout=7, check=False)
                    result["sampleExit"] = completed.returncode
                except subprocess.TimeoutExpired:
                    timed_out = True
            if not same_owner(processes, child, parent, browser, guard):
                return dict(result, status="identity-changed-after")
            result["stderr"] = retain_bounded(diagnostic, output.with_suffix(".stderr.log"), STDERR_LIMIT)
            stack = retain_bounded(raw, output, LIMIT)
            if timed_out:
                # Keep a partial diagnostic only after identity readback. It
                # remains a timeout, never a successful sample or native pass.
                return dict(result, status="sample-error", errorType="TimeoutExpired", partialStack=stack)
            if result["sampleExit"] != 0 or stack is None:
                result["partialStack"] = stack
                return dict(result, status="sample-unavailable")
            return dict(result, status="captured", **stack)
    except Exception as error:
        # Diagnostics are best effort, including a missing sample executable.
        # Do not retain subprocess arguments or arbitrary exception messages.
        return dict(result, status="sample-error", errorType=type(error).__name__)


def _emit(record):
    """The private worker reports metadata only; a full pipe is not a wait."""
    try:
        data = (json.dumps(record) + "\n").encode()
        if len(data) <= 4096:
            os.write(1, data)
    except (OSError, ValueError):
        pass


def _arm_group_deadline(seconds):
    """A fresh helper session owns this timer and every diagnostic command.

    The independent timer can kill that session even if its worker is stuck
    in file/libproc IO. The existing App/browser/guard are never in it.
    This function runs only in the new diagnostic process. The timer execs
    a fresh interpreter instead of forking Python state on macOS.
    """
    group = os.getpid()
    if os.getpgrp() != group or not 0 < seconds <= DIAGNOSTIC_BUDGET:
        raise ValueError("Expected the owned diagnostic session")
    program = """import os,sys,time
time.sleep(max(0,float(sys.argv[2])-time.monotonic()))
try:
    os.write(1,b'{"diagnosticOnly":true,"status":"diagnostic-deadline"}\\n')
except OSError:
    pass
os.killpg(int(sys.argv[1]),9)
"""
    return subprocess.Popen([sys.executable, "-B", "-c", program, str(group),
                             str(time.monotonic() + seconds)],
                            stdin=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def _image_spec(image):
    return {"path": str(image.file.name), "identity": list(image.identity)}


def _launch_command(command):
    global _active_diagnostic
    if _active_diagnostic is not None:
        raise RuntimeError("The owned diagnostic helper has not retired")
    # Keep this Popen object unreaped until _retire has signalled its own
    # group. The unreaped leader pins the PID against a reused-group kill.
    process = subprocess.Popen(command,
                               stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.DEVNULL, start_new_session=True,
                               env={"PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
                                    "PYTHONDONTWRITEBYTECODE": "1"})
    _active_diagnostic = process
    process._diagnostic_nonblocking = False
    try:
        os.set_blocking(process.stdout.fileno(), False)
        process._diagnostic_nonblocking = True
    except Exception as setup_error:
        try:
            _, receipt = _retire(process)
        except Exception as retirement_error:
            receipt = {"diagnosticOnly": True, "status": "diagnostic-retired",
                       "cleanupComplete": False, "errorType": type(retirement_error).__name__}
        if not receipt["cleanupComplete"]:
            raise DiagnosticRetirementError(receipt) from setup_error
        raise
    return process


def _launch(spec):
    return _launch_command([sys.executable, "-B", __file__, json.dumps(spec)])


def _read_reply(process):
    if getattr(process, "_diagnostic_nonblocking", None) is not True:
        return []  # Setup failed: discard metadata instead of a blocking read.
    data = bytearray()
    while len(data) < REPLY_LIMIT:
        try:
            block = os.read(process.stdout.fileno(), REPLY_LIMIT - len(data))
        except BlockingIOError:
            break
        if not block:
            break
        data.extend(block)
    records = []
    for line in data.splitlines():
        try:
            record = json.loads(line)
            if isinstance(record, dict):
                records.append(record)
        except ValueError:
            records.append({"diagnosticOnly": True, "status": "partial-reply"})
    return records


def _retire(process):
    """Kill only the new diagnostic group, then reap and verify exhaustion.

    No poll/communicate/wait may precede the signal: the held leader is the
    ownership anchor. After reaping, group probes are read-only, never kills.
    """
    global _active_diagnostic
    if process is not _active_diagnostic:
        raise ValueError("Expected the currently owned diagnostic helper")
    end = time.monotonic() + RETIRE_BUDGET
    error = None
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    except OSError as failure:
        error = type(failure).__name__
    reaped = False
    try:
        process.wait(timeout=max(0, end - time.monotonic()))
        reaped = True
    except subprocess.TimeoutExpired:
        error = "TimeoutExpired"
    gone = False
    while reaped and time.monotonic() < end:
        try:
            os.killpg(process.pid, 0)
        except ProcessLookupError:
            gone = True
            break
        except OSError as failure:
            error = type(failure).__name__
            break
        time.sleep(0.01)
    records = _read_reply(process)
    process.stdout.close()
    if reaped and gone:
        _active_diagnostic = None
    return records, {"diagnosticOnly": True, "status": "diagnostic-retired",
                     "cleanupComplete": reaped and gone, "leaderReaped": reaped,
                     "groupGone": gone, "exitCode": process.returncode, "errorType": error,
                     "replyNonblocking": getattr(process, "_diagnostic_nonblocking", None) is True}


def _spec(browser, guard, targets, mode):
    return {"mode": mode, "budget": DIAGNOSTIC_BUDGET,
            "browser": _image_spec(browser), "guard": _image_spec(guard), "targets": targets}


def sample_owned(processes, child, parent, browser, guard, output):
    """The late failure probe uses the same bounded owner as in-flow probes.

    Native identity and file checks are reconstructed inside the isolated
    worker; no possibly blocking target inspection runs on the UI runner.
    """
    target = {"child": child, "parent": parent, "output": str(output)}
    try:
        process = _launch(_spec(browser, guard, [target], "sample"))
    except DiagnosticRetirementError as error:
        return dict(error.receipt, pid=child["pid"], start=child["start"],
                    status="diagnostic-cleanup-incomplete")
    end = time.monotonic() + DIAGNOSTIC_BUDGET
    records = []
    try:
        while time.monotonic() < end and not records:
            readable, _, _ = select.select([process.stdout], [], [], max(0, end - time.monotonic()))
            if readable:
                records.extend(_read_reply(process))
    finally:
        trailing, receipt = _retire(process)
        records.extend(trailing)
    if not receipt["cleanupComplete"]:
        return dict(receipt, pid=child["pid"], start=child["start"], status="diagnostic-cleanup-incomplete")
    return next((record for record in records if record.get("pid") == child["pid"]),
                {"pid": child["pid"], "start": child["start"], "diagnosticOnly": True,
                 "status": "sample-unavailable", "retirement": receipt})


@contextmanager
def watch_shutdowns(observer, pages):
    """Opt-in, time-bounded helper; no runner thread can hold up cleanup."""
    if getattr(observer, "sample_shutdowns", False) is not True:
        yield
        return
    if getattr(observer, "_shutdown_diagnostic", None) is not None:
        observer.proof.setdefault("shutdownSamples", []).append(
            {"diagnosticOnly": True, "status": "diagnostic-busy"})
        yield
        return
    identities = {item["pid"]: item for item in observer.identities}
    phase = observer.phase or "first"
    targets = [{"child": identities[page.native["nativePid"]],
                "parent": identities[page.process["pid"]],
                "log": str(Path(page.process["directory"]) / "browser.log"),
                "output": str(observer.data / "evidence" / f"shutdown-{phase}"
                              / f"sample-{page.native['nativePid']}.log")} for page in pages]
    try:
        if not 1 <= len(targets) <= 2:
            raise ValueError("Expected one or two owned diagnostic targets")
        process = _launch(_spec(observer.browser, observer.guard, targets, "watch"))
    except DiagnosticRetirementError as error:
        observer._shutdown_diagnostic = _active_diagnostic
        observer.proof.setdefault("shutdownDiagnosticRetirements", []).append(error.receipt)
        raise
    except Exception as error:
        observer.proof.setdefault("shutdownSamples", []).append(
            {"diagnosticOnly": True, "status": "worker-start-error", "errorType": type(error).__name__})
        yield
        return
    observer._shutdown_diagnostic = process
    try:
        yield
    finally:
        original_failure = sys.exc_info()[1]
        records, receipt = _retire(process)
        observer.proof.setdefault("shutdownSamples", []).extend(records)
        observer.proof.setdefault("shutdownDiagnosticRetirements", []).append(receipt)
        if receipt["cleanupComplete"]:
            observer._shutdown_diagnostic = None
        else:
            # Keep the handle and refuse another launch. The original guard
            # cleanup still runs as this error unwinds the runner's finally.
            if original_failure is not None:
                original_failure.add_note("The owned diagnostic helper did not retire")
            else:
                raise RuntimeError("The owned diagnostic helper did not retire")


def _worker(spec):
    os.set_blocking(1, False)
    timer = _arm_group_deadline(spec["budget"])
    # Import native/file adapters only after the independent deadline is armed.
    from macos_browser_process import Processes
    from macos_process_image import PinnedImage
    if spec["mode"] not in ("sample", "watch") or not 1 <= len(spec["targets"]) <= 2:
        raise ValueError("Invalid private diagnostic request")
    with PinnedImage(Path(spec["browser"]["path"])) as browser, \
            PinnedImage(Path(spec["guard"]["path"])) as guard:
        if list(browser.identity) != spec["browser"]["identity"] or list(guard.identity) != spec["guard"]["identity"]:
            raise ValueError("The pinned diagnostic images changed")
        processes = Processes()
        first_seen, attempted = {}, set()
        while True:
            for target in spec["targets"]:
                child, parent = target["child"], target["parent"]
                pid = child["pid"]
                if pid in attempted:
                    continue
                if spec["mode"] == "watch":
                    if not shutdown_pending(Path(target["log"]), child):
                        first_seen.pop(pid, None)
                        continue
                    first_seen.setdefault(pid, time.monotonic())
                    if time.monotonic() - first_seen[pid] < 2:
                        continue
                attempted.add(pid)
                _emit(_sample_owned(processes, child, parent, browser, guard, Path(target["output"])))
            time.sleep(0.1)


if __name__ == "__main__":
    try:
        if len(sys.argv) != 2 or len(sys.argv[1]) > 65536:
            raise ValueError("Invalid private diagnostic request")
        _worker(json.loads(sys.argv[1]))
    except Exception as error:
        _emit({"diagnosticOnly": True, "status": "worker-error", "errorType": type(error).__name__})
