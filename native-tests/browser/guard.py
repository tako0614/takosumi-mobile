#!/usr/bin/env python3
"""Own one test browser and reap its detached helpers before profile deletion."""
import ctypes
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "desktop"))
from web_home import descendants, process_identity


def ancestry_identity(pid):
    try:
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
        return fields[19], int(fields[1])
    except (FileNotFoundError, ProcessLookupError):
        return None


def is_owned(pid, parent, owner_start):
    # A PID-only discovery can become stale before the first identity read.
    # Recheck a stable ancestry chain to this exact live subreaper, including
    # detached helpers that have already been adopted. No argv/UID scan.
    chain = []
    current = pid
    for _ in range(128):
        identity = ancestry_identity(current)
        if identity is None:
            return False
        chain.append((current, identity))
        if current == parent:
            return identity[0] == owner_start and all(ancestry_identity(item) == expected for item, expected in reversed(chain))
        if identity[1] <= 1 or identity[1] in {item for item, _ in chain}:
            return False
        current = identity[1]
    return False


class ProcessHandles:
    def __init__(self):
        self.handles = {}

    def observe(self, parent):
        owner = process_identity(parent)
        if owner is None:
            raise RuntimeError("Browser guard identity is unavailable")
        for pid in descendants(parent):
            if pid == parent:
                continue
            identity = process_identity(pid)
            if not identity:
                continue
            previous = self.handles.get(pid)
            if previous and previous[0] == identity[0]:
                continue
            if previous:
                os.close(previous[1])
                del self.handles[pid]
            try:
                fd = os.pidfd_open(pid)
            except ProcessLookupError:
                continue
            retained = False
            try:
                current = process_identity(pid)
                if current and current[0] == identity[0] and is_owned(pid, parent, owner[0]):
                    self.handles[pid] = (identity[0], fd)
                    retained = True
            finally:
                if not retained:
                    os.close(fd)


def main():
    receipt = Path(sys.argv[1])
    assert receipt.is_absolute() and not receipt.exists()
    assert os.getpgrp() == os.getpid(), "Browser guard needs its own session"
    # Test capability and reserve the receipt before any browser can exist.
    probe = os.pidfd_open(os.getpid())
    try:
        signal.pidfd_send_signal(probe, 0)
        descendants(os.getpid())
    finally:
        os.close(probe)
    output = receipt.open("x")
    os.chmod(receipt, 0o600)
    libc = ctypes.CDLL(None, use_errno=True)
    parent = os.getppid()
    stopping = False

    def stop(_signum, _frame):
        nonlocal stopping
        stopping = True

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    for option, value in ((36, 1), (1, signal.SIGTERM)):
        if libc.prctl(option, value, 0, 0, 0):
            output.close()
            raise OSError(ctypes.get_errno(), "Cannot guard owned browser lifecycle")
    if os.getppid() != parent:
        stopping = True
    child = None
    root_exit = None
    tracker = ProcessHandles()
    handles = tracker.handles
    forced = set()
    failure = None
    cleanup_errors = set()

    def observe():
        tracker.observe(os.getpid())

    def reap():
        """ECHILD is the kernel proof; a cached empty list is insufficient."""
        nonlocal root_exit
        if child and root_exit is None and child.returncode is not None:
            root_exit = child.returncode
        while True:
            try:
                pid, status = os.waitpid(-1, os.WNOHANG)
            except ChildProcessError:
                return True
            if not pid:
                return False  # The kernel still has at least one live child.
            if child and pid == child.pid and root_exit is None:
                root_exit = os.waitstatus_to_exitcode(status)
                child.returncode = root_exit

    def alive():
        return [pid for pid, (start, _) in handles.items()
                if (identity := process_identity(pid)) and identity[0] == start]

    def safe_observe():
        try:
            observe()
        except Exception as error:
            cleanup_errors.add(type(error).__name__)

    def safe_reap():
        try:
            return reap()
        except Exception as error:
            cleanup_errors.add(type(error).__name__)
            return False

    def send(signum):
        safe_observe()
        for pid, (_, fd) in list(handles.items()):
            try:
                signal.pidfd_send_signal(fd, signum)
                forced.add(pid)
            except ProcessLookupError:
                pass
            except Exception as error:
                cleanup_errors.add(type(error).__name__)
        # If observation failed before its first pidfd, the unreaped direct
        # child is still owned by Popen. Its send_signal checks/reaps that exact
        # child before signalling, without borrowing a process/group elsewhere.
        if child and root_exit is None:
            try:
                child.send_signal(signum)
            except ProcessLookupError:
                pass
            except Exception as error:
                cleanup_errors.add(type(error).__name__)

    empty = False
    try:
        try:
            child = subprocess.Popen(sys.argv[2:])
            deadline = time.monotonic() + 120
            while root_exit is None and not stopping and time.monotonic() < deadline:
                observe()
                reap()
                time.sleep(0.02)
        except BaseException as error:
            failure = type(error).__name__
        finally:
            # Every post-spawn error takes this same bounded drain path.
            for signum, budget in ((None, 0.5), (signal.SIGTERM, 0.5), (signal.SIGKILL, 2)):
                if signum is not None:
                    send(signum)
                deadline = time.monotonic() + budget
                while time.monotonic() < deadline:
                    safe_observe()
                    empty = safe_reap()
                    if empty:
                        break
                    if signum is not None:
                        send(signum)  # Includes newly adopted late-fork helpers.
                    time.sleep(0.02)
                if empty:
                    break
            # No child can fork after ECHILD: any living descendant would have
            # a direct-child ancestor or already be adopted by this subreaper.
            empty = safe_reap()
        remaining = alive()
        result = {"rootExit": root_exit, "complete": bool(child) and root_exit is not None and empty,
                  "remaining": remaining, "kernelNoChildren": empty, "observed": len(handles),
                  "forcedCleanup": len(forced), "error": failure, "cleanupErrors": sorted(cleanup_errors)}
        json.dump(result, output)
        output.flush()
        return 0 if result["complete"] and root_exit == 0 and failure is None and not cleanup_errors else 1
    finally:
        output.close()
        for _, fd in handles.values():
            os.close(fd)


if __name__ == "__main__":
    sys.exit(main())
