"""Exercise real owned child processes; never signal a foreign process."""
import os
import subprocess
import sys
import time
import unittest

import owned_process


class OwnedProcessTests(unittest.TestCase):
    def child(self, ignore_term=False):
        script = "import signal,time; "
        if ignore_term:
            script += "signal.signal(signal.SIGTERM, signal.SIG_IGN); "
        script += "print('ready',flush=True); time.sleep(60)"
        child = subprocess.Popen([sys.executable, "-u", "-c", script], stdout=subprocess.PIPE, text=True)
        self.addCleanup(self.reap, child)
        self.assertEqual(child.stdout.readline().strip(), "ready")
        return child

    @staticmethod
    def reap(child):
        if child.poll() is None:
            child.kill()
        child.wait(timeout=3)
        child.stdout.close()

    def test_normal_child_stops_and_is_reapable(self):
        child = self.child()
        started = owned_process.birth(child.pid, os.getpid())
        owned_process.stop(child.pid, started)
        self.assertEqual(child.wait(timeout=1), -15)

    def test_term_ignoring_child_is_killed_within_bound(self):
        child = self.child(ignore_term=True)
        started = owned_process.birth(child.pid, os.getpid())
        before = time.monotonic()
        owned_process.stop(child.pid, started)
        self.assertEqual(child.wait(timeout=1), -9)
        self.assertLess(time.monotonic() - before, 6)

    def test_wrong_birth_and_parent_leave_the_child_alive(self):
        child = self.child()
        started = owned_process.birth(child.pid, os.getpid())
        with self.assertRaisesRegex(RuntimeError, "launching shell"):
            owned_process.birth(child.pid, os.getpid() + 1)
        with self.assertRaisesRegex(RuntimeError, "different process birth"):
            owned_process.stop(child.pid, str(int(started) + 1))
        self.assertIsNone(child.poll())

    def test_already_exited_child_needs_no_signal(self):
        child = self.child()
        started = owned_process.birth(child.pid, os.getpid())
        child.terminate()
        child.wait(timeout=3)
        owned_process.stop(child.pid, started)
        self.assertEqual(owned_process.birth(child.pid, os.getpid()), "exited")
        owned_process.stop(child.pid, "exited")


if __name__ == "__main__":
    unittest.main()
