#!/usr/bin/env python3
"""Linux process controls; every subreaper lives in its own disposable worker."""

import os
import contextlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

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
import ctypes,os,signal,sys,time
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
%s
%s
%s
""" % ("while not (root/'observed').exists(): time.sleep(0.01)" if mode == "late-error" else "pass",
       "WebKitNetworkPr" if network else "WebKitWebProces",
       "while not (root/'observed').exists(): time.sleep(0.01)" if mode == "parent-reaped" else "pass",
       "os.kill(os.getpid(), signal.SIGTERM)" if mode in ("signal", "renderer-signal")
       else f"os._exit({7 if mode in ('error', 'late-error', 'parent-reaped') else 0})",
       "os.waitpid(pid,0)" if mode == "parent-reaped" else "time.sleep(0.10)",
       "print('PASS: completed profile reset through shared UI and real native IPC preserves other profiles', flush=True)" if mode in ("missing-after-reset", "missing-reset-identity", "reset-restart") else "pass",
       f"print({'FIXTURE: post-reset connection preferences ' + json.dumps('synthetic prefs')!r}, flush=True)" if mode in ("missing-after-reset", "reset-restart") else "pass",
       "if '--after-reset' in sys.argv:\n    assert os.environ.get('TAKOSUMI_FIXTURE_EXPECTED_CONNECTIONS') == 'synthetic prefs'\n    print('PASS: post-reset App restart preserves deletion and other profiles', flush=True)" if mode == "reset-restart" else
       "if '--after-reset' in sys.argv: print('PASS: post-reset App restart preserves deletion and other profiles', flush=True)" if mode == "missing-reset-identity" else "pass"))
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
        elif mode in ("missing-reset", "missing-reset-before-restart"):
            try:
                web_home.run_apps(fake, root, False, False, profile_reset=mode == "missing-reset",
                                  reset_restart=mode == "missing-reset-before-restart")
            except RuntimeError as error:
                assert "Profile reset case did not report" in str(error), error
                assert not (root / ("readback.log" if mode == "missing-reset" else "after-reset.log")).exists()
            else:
                raise AssertionError("An old App that ignored reset mode was accepted")
        elif mode == "missing-after-reset":
            try:
                web_home.run_apps(fake, root, False, False, reset_restart=True)
            except RuntimeError as error:
                assert "Post-reset restart case did not report" in str(error), error
                assert (root / "after-reset.log").exists()
            else:
                raise AssertionError("An old App that ignored after-reset mode was accepted")
        elif mode == "missing-reset-identity":
            try:
                web_home.run_apps(fake, root, False, False, reset_restart=True)
            except RuntimeError as error:
                assert "Reset restart did not report exact connection preferences" in str(error), error
                assert not (root / "after-reset.log").exists()
            else:
                raise AssertionError("Reset restart accepted without the cleared profile's exact identity")
        elif mode == "reset-restart":
            web_home.run_apps(fake, root, False, False, reset_restart=True)
            assert (root / "after-reset.log").exists()
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
    def test_native_root_exit_observation_preserves_cleanup_result_and_io(self):
        import run as native_runner
        prefix = "NATIVE_ROOT_EXIT_OBSERVATION "
        # The actual worker runs only against cached fake Popen attributes here.
        # No native child, process lookup, clock, signal or storage operation runs.
        for code in (0, 17, -6, None, True, "SYNTHETIC_PRIVATE_STATUS"):
            for cleanup_fails in (False, True):
                for sink_fails in (False, True):
                    with self.subTest(code_type=type(code).__name__, cleanup=cleanup_fails, sink=sink_fails):
                        child = mock.Mock(pid=1234, returncode=code)
                        child.poll.return_value = 0
                        records, markers, order = [], [], []
                        original = RuntimeError("SYNTHETIC_PRIVATE_CLEANUP")
                        def cleanup(*_args):
                            order.append("cleanup")
                            if cleanup_fails:
                                raise original
                            return {}
                        def emit(*parts, **options):
                            text = " ".join(map(str, parts))
                            if text.startswith(prefix):
                                order.append("observation")
                                self.assertEqual(options, {"flush": True})
                                if sink_fails:
                                    raise OSError("SYNTHETIC_PRIVATE_SINK")
                                records.append(json.loads(text.removeprefix(prefix)))
                            else:
                                order.append("marker")
                                markers.append(text)
                        with mock.patch.object(native_runner, "enable_child_subreaper") as subreaper, \
                             mock.patch.object(native_runner.subprocess, "Popen", return_value=child) as spawn, \
                             mock.patch.object(native_runner, "observe_children") as observe, \
                             mock.patch.object(native_runner, "wait_engine_children", side_effect=cleanup) as wait, \
                             mock.patch.object(native_runner.time, "monotonic", return_value=10.0) as clock, \
                             mock.patch.object(native_runner.time, "sleep") as sleep, \
                             mock.patch.object(native_runner, "print", side_effect=emit, create=True):
                            if cleanup_fails:
                                with self.assertRaises(RuntimeError) as raised:
                                    native_runner.run_native_generation(["SYNTHETIC_PRIVATE_COMMAND"])
                                self.assertIs(raised.exception, original)
                            else:
                                self.assertIs(native_runner.run_native_generation(["SYNTHETIC_PRIVATE_COMMAND"]), code)
                        self.assertEqual(order[:2], ["observation", "cleanup"])
                        self.assertEqual(len(records), 0 if sink_fails else 1)
                        if not sink_fails:
                            expected = code if type(code) is int and -255 <= code <= 255 else None
                            self.assertEqual(records[0], {
                                "schema": "takosumi.native-root-exit-observation@v1", "diagnosticOnly": True,
                                "historicalOnly": True, "scope": "held_popen_child", "stage": "before_engine_children",
                                "pid": 1234, "returncode": expected})
                            self.assertNotIn("SYNTHETIC_PRIVATE", json.dumps(records))
                        self.assertEqual(len(markers), 0 if cleanup_fails else 2)
                        self.assertEqual(child.poll.call_count, 2, "No observation poll may be added")
                        child.wait.assert_not_called(); child.terminate.assert_not_called(); child.kill.assert_not_called()
                        subreaper.assert_called_once_with(); spawn.assert_called_once_with(["SYNTHETIC_PRIVATE_COMMAND"])
                        observe.assert_not_called(); wait.assert_called_once_with({}, {})
                        clock.assert_called_once_with(); sleep.assert_not_called()

    def test_reset_restart_requires_the_cleared_profile_identity(self):
        self.run_control("missing-reset-identity")

    def test_reset_restart_requires_clear_before_starting_the_last_app(self):
        self.run_control("missing-reset-before-restart")

    def test_reset_restart_rejects_a_generic_last_app_success(self):
        self.run_control("missing-after-reset")

    def test_reset_restart_accepts_three_qualified_generations(self):
        self.run_control("reset-restart")

    def test_reset_mode_rejects_a_generic_success_without_reset_assertions(self):
        self.run_control("missing-reset")

    def test_timeout_diagnostics_and_cleanup_stay_in_the_owned_session(self):
        other = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"], start_new_session=True)
        output = io.StringIO()
        observed = []
        starts = {}
        describe = web_home.shutdown_description

        def observe(pid):
            observed.append(pid)
            details = describe(pid)
            starts[pid] = details["identity"][0]
            return details

        try:
            with mock.patch.object(web_home, "shutdown_description", side_effect=observe), contextlib.redirect_stderr(output):
                with self.assertRaises(subprocess.TimeoutExpired):
                    web_home.run_desktop_session([sys.executable, "-c", "import time; time.sleep(30) # SYNTHETIC_PRIVATE_ARG"],
                                                 dict(os.environ), timeout=0.3)
            self.assertTrue(observed)
            self.assertNotIn(other.pid, observed)
            self.assertIsNone(other.poll(), "Timeout cleanup signalled another session")
            for pid in observed:
                identity = web_home.process_identity(pid)
                self.assertTrue(identity is None or identity[0] != starts[pid], "The timed-out target survived cleanup")
            self.assertIn("Owned desktop session timeout:", output.getvalue())
            self.assertNotIn("SYNTHETIC_PRIVATE_ARG", output.getvalue())
        finally:
            other.terminate()
            other.wait(timeout=5)

    def test_shutdown_description_names_descriptor_kinds_without_paths(self):
        with tempfile.TemporaryDirectory(prefix="sumi-fds-") as temporary:
            log = Path(temporary) / "SYNTHETIC_PRIVATE_PATH.log"
            with log.open("w") as output:
                child = subprocess.Popen([sys.executable, "-c", "import sys; sys.stdin.read()"],
                                         stdin=subprocess.PIPE, stdout=output, stderr=subprocess.DEVNULL)
            try:
                pipe = os.fstat(child.stdin.fileno()).st_ino
                details = web_home.shutdown_description(child.pid)
            finally:
                child.stdin.close()
                child.wait(timeout=5)
            text = json.dumps(details)
            self.assertEqual(details["fds"], {"0": f"pipe:[{pipe}]", "1": "file", "2": "char"})
            self.assertNotIn("SYNTHETIC_PRIVATE_PATH", text)
            self.assertNotIn(temporary, text)
            self.assertNotIn("/dev/null", text)

    def test_still_alive_diagnostics_show_the_childs_and_the_workers_descriptors(self):
        # A child that outlives the 5 s bound: the failure names its standard
        # descriptors and this worker's stdout/stderr for comparison.
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"],
                                 stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        try:
            pipe = os.fstat(child.stdout.fileno()).st_ino
            tracked = {child.pid: web_home.process_identity(child.pid)[0]}
            descriptions = {child.pid: web_home.process_description(child.pid)}
            with self.assertRaises(RuntimeError) as raised:
                web_home.wait_engine_children(tracked, descriptions)
            message = str(raised.exception)
            self.assertIn("Old App engine children still alive", message)
            self.assertIn(f"'1': 'pipe:[{pipe}]'", message)
            self.assertIn("worker descriptors: {'1': ", message)
        finally:
            child.kill()
            child.wait(timeout=5)
            child.stdout.close()

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
