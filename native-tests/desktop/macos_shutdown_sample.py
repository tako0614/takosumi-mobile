"""Fixture-only, bounded stack evidence for an owned Mac browser in CefShutdown.

Sampling may perturb scheduling. It never changes the runner's timeouts or
receipt verdict and is not an unsampled qualification run. No process-name
search, signals, heap dump, environment or page data is requested.
"""
from contextlib import contextmanager
import json
from pathlib import Path
import subprocess
import tempfile
import threading
import time

LIMIT = 4 * 1024 * 1024
PREFIX = "takosumi-cef-mac-lifecycle: "


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
            and processes.image(child["pid"]) == browser
            and processes.image(parent["pid"]) == guard
            and processes.facts(child["pid"]) == child)


def sample_owned(processes, child, parent, browser, guard, output):
    """One one-second sample; discard output if process identity changed meanwhile."""
    result = {"pid": child["pid"], "start": child["start"], "diagnosticOnly": True}
    try:
        if not same_owner(processes, child, parent, browser, guard):
            return dict(result, status="identity-changed-before")
        output.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        # sample owns only its new temporary output. A timeout kills/reaps
        # that subprocess through subprocess.run, never the sampled browser.
        with tempfile.TemporaryDirectory(prefix="sample-", dir=output.parent) as scratch:
            raw = Path(scratch) / "stack.log"
            completed = subprocess.run(["/usr/bin/sample", str(child["pid"]), "1", "100", "-file", str(raw)],
                                       stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                       stderr=subprocess.DEVNULL, timeout=3, check=False)
            result["sampleExit"] = completed.returncode
            if not same_owner(processes, child, parent, browser, guard):
                return dict(result, status="identity-changed-after")
            if completed.returncode != 0 or not raw.is_file() or raw.is_symlink():
                return dict(result, status="sample-unavailable")
            with raw.open("rb") as stream:
                data = stream.read(LIMIT + 1)
            with output.open("xb") as stream:
                stream.write(data[:LIMIT])
            return dict(result, status="captured", bytes=min(len(data), LIMIT), truncated=len(data) > LIMIT)
    except Exception as error:
        # Diagnostics are best effort, including a missing sample executable.
        # Do not retain subprocess arguments or arbitrary exception messages.
        return dict(result, status="sample-error", errorType=type(error).__name__)


@contextmanager
def watch_shutdowns(observer, pages):
    """Watch during the existing close/wait, including a blocking Page.close reply."""
    identities = {item["pid"]: item for item in observer.identities}
    targets = [(identities[page.native["nativePid"]], identities[page.process["pid"]],
                Path(page.process["directory"]) / "browser.log") for page in pages]
    stopped = threading.Event()
    records = []
    phase = observer.phase or "first"

    def watch():
        first_seen, attempted = {}, set()
        while not stopped.wait(0.1):
            for child, parent, log in targets:
                if stopped.is_set():
                    return
                pid = child["pid"]
                if pid in attempted:
                    continue
                try:
                    if not shutdown_pending(log, child):
                        first_seen.pop(pid, None)
                        continue
                    first_seen.setdefault(pid, time.monotonic())
                    if time.monotonic() - first_seen[pid] < 2:
                        continue
                    attempted.add(pid)
                    output = observer.data / "evidence" / f"shutdown-{phase}" / f"sample-{pid}.log"
                    records.append(sample_owned(observer.processes, child, parent,
                                                observer.browser, observer.guard, output))
                except Exception as error:
                    attempted.add(pid)
                    records.append({"pid": pid, "status": "watch-error", "errorType": type(error).__name__})

    worker, started = None, False
    try:
        worker = threading.Thread(target=watch, name="owned-mac-shutdown-sample")
        worker.start()
        started = True
    except Exception as error:
        records.append({"status": "thread-start-error", "errorType": type(error).__name__})
    try:
        yield
    finally:
        stopped.set()
        if started:
            # Join before any fixture cleanup. The sample subprocess is
            # bounded, but filesystem/libproc reads have no hard IO deadline.
            worker.join()
        observer.proof.setdefault("shutdownSamples", []).extend(records)
        try:
            (observer.data / f"shutdown-samples-{phase}.json").write_text(
                json.dumps(observer.proof["shutdownSamples"], indent=2) + "\n")
        except OSError as error:
            observer.proof["shutdownSampleWriteError"] = type(error).__name__
