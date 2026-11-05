#!/usr/bin/env python3
"""Linux process controls; every subreaper lives in its own disposable worker."""

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

sys.dont_write_bytecode = True
import web_home


def worker(root, mode):
    web_home.enable_child_subreaper()
    if mode == "identity":
        pid = os.fork()
        if pid == 0:
            os._exit(0)
        try:
            deadline = time.monotonic() + 2
            while (identity := web_home.process_identity(pid))[1] != "Z":
                assert time.monotonic() < deadline
                time.sleep(0.01)
            statuses = {}
            web_home.reap_observed_children({pid: identity[0] + "-different"}, {pid: {}}, statuses)
            assert not statuses
            # The mismatched identity must not have been reaped.
            assert os.waitpid(pid, 0) == (pid, 0)
        finally:
            try:
                os.waitpid(pid, 0)
            except ChildProcessError:
                pass
        return
    network = mode != "renderer-signal"
    acknowledged = root / "observed"
    helper_pid = root / "helper.pid"
    if mode in ("late-error", "parent-reaped"):
        describe = web_home.process_description

        def acknowledge_observation(pid):
            details = describe(pid)
            if helper_pid.exists() and helper_pid.read_text() == str(pid):
                expected_role = mode == "parent-reaped"
                if web_home.is_network_process(details) == expected_role:
                    acknowledged.touch()
            return details

        web_home.process_description = acknowledge_observation
    fake = root / "fake-app"
    fake.write_text("""#!/usr/bin/env python3
import ctypes,os,signal,time
from pathlib import Path
root=Path(__file__).parent
pid=os.fork()
if pid==0:
    libc=ctypes.CDLL(None)
    (root/'helper.pid').write_text(str(os.getpid()))
    %s
    libc.prctl(15, b'%s', 0, 0, 0)
    %s
    time.sleep(0.25)
    %s
%s
print('PASS: full App Web-home flow, synthetic process control', flush=True)
""" % ("while not (root/'observed').exists(): time.sleep(0.01)" if mode == "late-error" else "pass",
       "WebKitNetworkPr" if network else "WebKitWebProces",
       "while not (root/'observed').exists(): time.sleep(0.01)" if mode == "parent-reaped" else "pass",
       "os.kill(os.getpid(), signal.SIGTERM)" if mode in ("signal", "renderer-signal")
       else f"os._exit({7 if mode in ('error', 'late-error', 'parent-reaped') else 0})",
       "os.waitpid(pid,0)" if mode == "parent-reaped" else "time.sleep(0.10)"))
    fake.chmod(0o700)
    try:
        if mode in ("error", "signal", "late-error"):
            try:
                web_home.run_apps(fake, root, False, False)
            except RuntimeError as error:
                assert "NetworkProcess exited abnormally" in str(error), error
                assert f"'status': {-15 if mode == 'signal' else 7}" in str(error), error
                assert not (root / "readback.log").exists()
            else:
                raise AssertionError("Abnormal NetworkProcess accepted as successful shutdown")
        else:
            web_home.run_apps(fake, root, False, False)
            assert (root / "readback.log").exists()
    finally:
        # Only this synthetic worker's own children can be adopted here.
        while True:
            try:
                os.waitpid(-1, 0)
            except ChildProcessError:
                break


class ProcessLifecycleTests(unittest.TestCase):
    def run_control(self, mode):
        with tempfile.TemporaryDirectory(prefix="sumi-process-test-") as temporary:
            result = subprocess.run([sys.executable, __file__, "--worker", temporary, mode],
                                    stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                    text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stdout)
            return result.stdout

    def test_normal_network_exit_allows_new_process_readback(self):
        self.run_control("normal")

    def test_network_error_refuses_readback(self):
        self.run_control("error")

    def test_network_signal_refuses_readback(self):
        self.run_control("signal")

    def test_renderer_signal_is_not_a_network_flush(self):
        self.run_control("renderer-signal")

    def test_changed_pid_identity_is_not_reaped(self):
        self.run_control("identity")

    def test_network_role_after_first_observation_refuses_readback(self):
        self.run_control("late-error")

    def test_parent_reaped_status_is_reported_unknown(self):
        output = self.run_control("parent-reaped")
        self.assertEqual(output.count("Unknown NetworkProcess exit status"), 2, output)


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--worker":
        worker(Path(sys.argv[2]), sys.argv[3])
    else:
        unittest.main()
