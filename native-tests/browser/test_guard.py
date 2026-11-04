#!/usr/bin/env python3
"""Synthetic browser lifecycle controls; no existing sessions or user data."""
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

sys.dont_write_bytecode = True
GUARD = Path(__file__).with_name("guard.py")
spec = importlib.util.spec_from_file_location("owned_guard", GUARD)
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


def terminate_record(path):
    if not path.exists():
        return
    pid, start = json.loads(path.read_text())
    try:
        fd = os.pidfd_open(pid)
    except ProcessLookupError:
        return
    try:
        identity = guard.process_identity(pid)
        if identity and identity[0] == start:
            signal.pidfd_send_signal(fd, signal.SIGKILL)
    except ProcessLookupError:
        pass
    finally:
        os.close(fd)


class BrowserGuardTests(unittest.TestCase):
    def control(self, mode):
        with tempfile.TemporaryDirectory(prefix="sumi-browser-guard-") as temporary:
            root = Path(temporary)
            other = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(10)"], start_new_session=True)
            command = """
import json,os,signal,sys,time
from pathlib import Path
root=Path(sys.argv[1]); mode=sys.argv[2]
def record(name):
    start=Path('/proc/self/stat').read_text().split(') ',1)[1].split()[19]
    (root/name).write_text(json.dumps([os.getpid(),start]))
pid=os.fork()
if pid==0:
    os.setsid()
    signal.signal(signal.SIGTERM,signal.SIG_IGN)
    if mode=='late':
        def late(sig,frame):
            signal.signal(signal.SIGTERM,signal.SIG_IGN)
            if os.fork()==0:
                os.setsid(); record('late.json'); time.sleep(5); os._exit(0)
            os._exit(0)
        signal.signal(signal.SIGTERM,late)
    record('helper.json')
    time.sleep(5); os._exit(0)
deadline=time.monotonic()+2
while not (root/'helper.json').exists():
    assert time.monotonic()<deadline
    time.sleep(0.01)
if mode=='stop': time.sleep(5)
if mode=='fault':
    (root/'root-exiting').touch()
    deadline=time.monotonic()+2
    while not (root/'fault-injected').exists():
        assert time.monotonic()<deadline
        time.sleep(0.01)
os._exit(0)
"""
            argv = [sys.executable, str(GUARD), str(root / "receipt.json"), sys.executable, "-c", command, str(root), mode]
            if mode == "fault":
                wrapper = """
import importlib.util,sys
from pathlib import Path
path=sys.argv[1]; root=Path(sys.argv[-2])
spec=importlib.util.spec_from_file_location('guard_fixture',path)
g=importlib.util.module_from_spec(spec); spec.loader.exec_module(g)
original=g.ProcessHandles.observe
failed=False
def fault(self,parent):
    global failed
    if (root/'root-exiting').exists() and not failed:
        failed=True; (root/'fault-injected').touch()
        raise OSError('Synthetic observer failure')
    return original(self,parent)
g.ProcessHandles.observe=fault
sys.argv=[path,*sys.argv[2:]]
sys.exit(g.main())
"""
                argv = [sys.executable, "-c", wrapper, *argv[1:]]
            process = subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, start_new_session=True)
            try:
                deadline = time.monotonic() + 3
                while not (root / "helper.json").exists():
                    self.assertLess(time.monotonic(), deadline)
                    time.sleep(0.01)
                if mode == "stop":
                    process.send_signal(signal.SIGTERM)
                output = process.communicate(timeout=6)[0]
                self.assertEqual(process.returncode, 1 if mode in ("stop", "fault") else 0, output)
                result = json.loads((root / "receipt.json").read_text())
                self.assertTrue(result["complete"], result)
                self.assertTrue(result["kernelNoChildren"], result)
                self.assertEqual(result["remaining"], [])
                self.assertGreaterEqual(result["observed"], 2)
                self.assertGreaterEqual(result["forcedCleanup"], 1)
                if mode == "fault":
                    self.assertEqual(result["rootExit"], 0)
                    self.assertEqual(result["error"], "OSError")
                if mode == "late":
                    self.assertTrue((root / "late.json").exists(), "Late-fork control did not execute")
                    self.assertGreaterEqual(result["observed"], 3)
                self.assertIsNone(other.poll(), "Another test session was signalled")
                for name in ("helper.json", "late.json"):
                    if (root / name).exists():
                        pid, start = json.loads((root / name).read_text())
                        identity = guard.process_identity(pid)
                        self.assertTrue(identity is None or identity[0] != start)
            finally:
                try:
                    if process.poll() is None:
                        process.send_signal(signal.SIGTERM)
                        process.wait(timeout=6)
                finally:
                    try:
                        for name in ("helper.json", "late.json"):
                            terminate_record(root / name)
                    finally:
                        other.terminate()
                        other.wait(timeout=5)

    def test_detached_orphan_is_reaped_after_normal_root_exit(self):
        self.control("normal")

    def test_cancellation_reaps_root_and_detached_helper(self):
        self.control("stop")

    def test_observer_failure_still_drains_and_returns_failure(self):
        self.control("fault")

    def test_late_fork_requires_kernel_no_children(self):
        self.control("late")

    def test_numeric_pid_generation_replaces_its_stale_handle(self):
        tracker = guard.ProcessHandles()
        generation = ["first"]
        def identity(pid):
            return ("owner", "S") if pid == 10 else (generation[0], "S")
        with mock.patch.object(guard, "descendants", return_value={42}), mock.patch.object(guard, "process_identity", side_effect=identity), \
                mock.patch.object(guard, "is_owned", return_value=True), mock.patch.object(guard.os, "pidfd_open", side_effect=[10001, 10002]) as opened, \
                mock.patch.object(guard.os, "close") as closed:
            tracker.observe(10)
            generation[0] = "second"
            tracker.observe(10)
            self.assertEqual(tracker.handles[42], ("second", 10002))
            self.assertEqual(opened.call_count, 2)
            closed.assert_called_once_with(10001)

    def test_stale_discovery_refuses_a_process_from_another_ancestry(self):
        owner = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(10)"], start_new_session=True)
        other = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(10)"], start_new_session=True)
        try:
            tracker = guard.ProcessHandles()
            with mock.patch.object(guard, "descendants", return_value={other.pid}):
                tracker.observe(owner.pid)
            self.assertEqual(tracker.handles, {}, "Stale PID discovery accepted an unrelated process")
            self.assertIsNone(other.poll())
        finally:
            try:
                owner.terminate()
                owner.wait(timeout=5)
            finally:
                other.terminate()
                other.wait(timeout=5)

    def test_unverified_handle_closes_when_ancestry_read_fails(self):
        tracker = guard.ProcessHandles()
        with mock.patch.object(guard, "descendants", return_value={42}), \
                mock.patch.object(guard, "process_identity", return_value=("start", "S")), \
                mock.patch.object(guard.os, "pidfd_open", return_value=10001), \
                mock.patch.object(guard.os, "close") as closed, \
                mock.patch.object(guard, "is_owned", side_effect=PermissionError("Synthetic ancestry failure")):
            with self.assertRaises(PermissionError):
                tracker.observe(10)
            self.assertEqual(tracker.handles, {})
            closed.assert_called_once_with(10001)

    def test_ancestry_parser_uses_the_final_process_name_delimiter(self):
        fields = Path("/proc/self/stat").read_text().rsplit(") ", 1)[1]
        expected = fields.split()
        # A process comm may itself contain ') '. It must not forge a parent
        # from that text when verifying an unrelated discovery snapshot.
        with mock.patch.object(guard.Path, "read_text", return_value=f"42 () S 999 ) {fields}"):
            self.assertEqual(guard.ancestry_identity(42), (expected[19], int(expected[1])))


if __name__ == "__main__":
    unittest.main()
