"""Harness controls only. These cannot qualify WKDownload or OS export."""
import copy
import http.client
import json
import os
from pathlib import Path
import ssl
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import Mock, patch

import run as fixture_runner

from run import (certificate, owned_device, require_interrupted_transfers,
                 require_test_pass, select_runtime, verify_exports)
from server import BLOB_BYTES, COOKIE_BYTES, SENTINEL_BYTES, FixtureServer


class HarnessTests(unittest.TestCase):
    def setUp(self):
        for name in ("UNREAPED_COMMANDS", "INTERRUPTED_COMMANDS"):
            reset = patch.object(fixture_runner, name, [])
            reset.start()
            self.addCleanup(reset.stop)

    def test_timeout_reaps_only_owned_cli_and_keeps_original_failure(self):
        original = subprocess.TimeoutExpired(["xcrun", "simctl", "bootstatus", "owned-id", "-b"], 180)
        process = Mock(pid=424242)
        process.wait.side_effect = [original, subprocess.TimeoutExpired(original.cmd, 5), -9]
        process.poll.return_value = None
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            path = Path(raw) / "bootstatus.log"
            # The first real run's SIGKILL to the group failed with EPERM.
            # A CLI timeout must neither signal platform helpers nor hide it.
            with patch.object(fixture_runner.subprocess, "Popen", return_value=process), \
                 patch.object(fixture_runner.os, "killpg", side_effect=[None, PermissionError(1, "not permitted")]) as group:
                with self.assertRaises(subprocess.TimeoutExpired) as caught:
                    fixture_runner.command(original.cmd, path, timeout=180)
                self.assertIs(caught.exception, original)
                group.assert_not_called()
                process.terminate.assert_called_once_with()
                process.kill.assert_called_once_with()
                result = json.loads(path.with_suffix(".process.json").read_text())
                self.assertTrue(result["reaped"])
                self.assertEqual(result["originalErrorType"], "TimeoutExpired")

    def test_real_timed_out_child_is_reaped(self):
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            with fixture_runner.owned_scratch(Path(raw)) as (scratch, state):
                path = scratch / "owned-child.log"
                with self.assertRaises(subprocess.TimeoutExpired):
                    fixture_runner.command([sys.executable, "-c", "import time; time.sleep(30)"], path, timeout=0.05)
                result = json.loads(path.with_suffix(".process.json").read_text())
                self.assertTrue(result["reaped"])
                with self.assertRaises(ChildProcessError):
                    os.waitpid(result["pid"], os.WNOHANG)
            # A direct child being gone is not a general descendant proof.
            self.assertTrue(state["retain"])
            self.assertTrue(scratch.is_dir())
            # This test's Python child spawned none and was reaped above, so
            # the test's outer TemporaryDirectory may retire its own fixture.

    def test_unreaped_cli_is_recorded_without_masking_timeout(self):
        original = subprocess.TimeoutExpired(["xcrun", "simctl", "shutdown", "owned-id"], 60)
        process = Mock(pid=424242)
        process.wait.side_effect = [original, subprocess.TimeoutExpired(original.cmd, 5),
                                    subprocess.TimeoutExpired(original.cmd, 5)]
        process.poll.return_value = None
        process.terminate.side_effect = PermissionError(1, "not permitted")
        process.kill.side_effect = PermissionError(1, "not permitted")
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            path = Path(raw) / "shutdown.log"
            with patch.object(fixture_runner.subprocess, "Popen", return_value=process), \
                 patch.object(fixture_runner, "UNREAPED_COMMANDS", []) as live:
                with self.assertRaises(subprocess.TimeoutExpired) as caught:
                    fixture_runner.command(original.cmd, path)
                self.assertIs(caught.exception, original)
                self.assertEqual(live, [{"process": process, "log": str(path)}])
                result = json.loads(path.with_suffix(".process.json").read_text())
                self.assertFalse(result["reaped"])
                self.assertEqual([s["errorType"] for s in result["signals"]], ["PermissionError"] * 2)

    def test_cold_boot_needs_recent_distinct_migration_not_just_elapsed_time(self):
        def entry(seconds, plugin):
            return (f"Status=2, isTerminal=NO, Elapsed={seconds // 60:02}:{seconds % 60:02}.\n"
                    f"Reason:Running plugin {plugin} (fixture.migrator)\n")
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            path = Path(raw) / "bootstatus.log"
            for log, allowed in ((entry(5, "launch") + entry(175, "launch"), False),
                                 (entry(5, "launch") + entry(20, "keys") + entry(175, "keys"), False),
                                 (entry(5, "launch") + entry(165, "safari") + entry(175, "safari"), True),
                                 ("Status=4, isTerminal=NO, Elapsed=02:55.\n", False), ("", False)):
                path.write_text(log)
                self.assertEqual(fixture_runner.migration_progress(path)["recentMigrationProgress"], allowed)

    def test_cold_boot_continues_same_process_once_and_requires_exit_zero(self):
        argv = ["xcrun", "simctl", "bootstatus", "owned-id", "-b"]
        log = (b"Status=2, isTerminal=NO, Elapsed=00:05.\nReason:Running plugin launch (a.migrator)\n"
               b"Status=2, isTerminal=NO, Elapsed=02:45.\nReason:Running plugin safari (b.migrator)\n"
               b"Status=2, isTerminal=NO, Elapsed=02:55.\nReason:Running plugin safari (b.migrator)\n")
        for final in (0, 1, subprocess.TimeoutExpired(argv, 120)):
            process = Mock(pid=424242)
            original = subprocess.TimeoutExpired(argv, 180)
            process.wait.side_effect = [original, final, -15]
            process.poll.return_value = None

            def spawn(*_, **kwargs):
                kwargs["stdout"].write(log)
                kwargs["stdout"].flush()
                return process

            with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
                path = Path(raw) / "bootstatus.log"
                with patch.object(fixture_runner.subprocess, "Popen", side_effect=spawn) as launch:
                    if final == 0:
                        self.assertEqual(fixture_runner.command(argv, path, timeout=180, boot_status=True), log.decode())
                    else:
                        with self.assertRaises((RuntimeError, subprocess.TimeoutExpired)):
                            fixture_runner.command(argv, path, timeout=180, boot_status=True)
                    launch.assert_called_once()
                    self.assertEqual([c.kwargs["timeout"] for c in process.wait.call_args_list[:2]], [180, 120])
                    self.assertEqual(json.loads(path.with_suffix(".progress.json").read_text())["extensionSeconds"], 120)

    def test_runtime_requires_installed_compatible_phone(self):
        runtime = {"identifier": "com.apple.CoreSimulator.SimRuntime.iOS-26-0", "version": "26.0",
                   "isAvailable": True, "supportedDeviceTypes": [{"name": "iPhone 16", "identifier": "phone-16"}]}
        self.assertEqual(select_runtime({"runtimes": [runtime]}), (runtime["identifier"], "phone-16"))
        for patch in ({"isAvailable": False}, {"version": "16.0"}, {"supportedDeviceTypes": []},
                      {"identifier": "com.apple.CoreSimulator.SimRuntime.tvOS-26-0"}):
            with self.assertRaises(RuntimeError):
                select_runtime({"runtimes": [dict(runtime, **patch)]})

    def test_deadline_boundary_requires_both_terminal_native_success_and_exit_zero(self):
        argv = ["xcrun", "simctl", "bootstatus", "owned-id", "-b"]
        # 2fd's native log ended this way, and retirement observed exit0
        # without sending any signal. Preserve that actual event ordering.
        waiting = "Status=4, isTerminal=NO, Elapsed=03:06.\n\tWaiting on System App\n"
        finished = "Status=4294967295, isTerminal=YES, Elapsed=03:10.\n\tFinished\n"
        cases = ((waiting + finished, 0, True), (waiting + finished, None, False),
                 (waiting + finished, 1, False), (waiting + "Finished\n", 0, False),
                 (finished.replace("Finished", "Failed"), 0, False), (finished + waiting, 0, False))
        for log, exit_code, accepted in cases:
            with self.subTest(log=log, exit_code=exit_code):
                process = Mock(pid=424242)
                original = subprocess.TimeoutExpired(argv, 180)
                process.wait.side_effect = [original, -15]
                process.poll.return_value = exit_code

                def spawn(*_, **kwargs):
                    kwargs["stdout"].write(log.encode())
                    kwargs["stdout"].flush()
                    return process

                with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
                    path = Path(raw) / "bootstatus.log"
                    with patch.object(fixture_runner.subprocess, "Popen", side_effect=spawn) as launch:
                        if accepted:
                            self.assertEqual(fixture_runner.command(argv, path, timeout=180, boot_status=True), log)
                            process.wait.assert_called_once_with(timeout=180)
                            process.terminate.assert_not_called()
                            process.kill.assert_not_called()
                            self.assertFalse(path.with_suffix(".process.json").exists())
                            proof = json.loads(path.with_suffix(".progress.json").read_text())
                            self.assertTrue(proof["terminalReady"])
                            self.assertEqual(proof["deadlineBoundaryExit"], 0)
                            self.assertEqual(proof["extensionSeconds"], 0)
                        else:
                            with self.assertRaises(subprocess.TimeoutExpired) as caught:
                                fixture_runner.command(argv, path, timeout=180, boot_status=True)
                            self.assertIs(caught.exception, original)
                        launch.assert_called_once()

    def test_cleanup_requires_exact_created_identity(self):
        device = {"udid": "id", "name": "owned", "state": "Booted"}
        inventory = {"devices": {"runtime": [device]}}
        self.assertIs(owned_device(inventory, "id", "owned", "runtime"), device)
        for identity in (("other", "owned", "runtime"), ("id", "other", "runtime"), ("id", "owned", "other")):
            with self.assertRaises(RuntimeError):
                owned_device(inventory, *identity)
        duplicate = copy.deepcopy(inventory)
        duplicate["devices"]["foreign"] = [device]
        with self.assertRaises(RuntimeError):
            owned_device(duplicate, "id", "owned", "runtime")

    def test_empty_skipped_or_failed_tests_never_qualify(self):
        method = "Test Case '-[DownloadUITests.DownloadUITests testOriginalResponsesExportAndRetireWithinTheirProfile]' passed (12 seconds)."
        aggregate = "Executed 1 test, with 0 failures (0 unexpected) in 12 seconds"
        success = "\n".join((method, aggregate, "** TEST SUCCEEDED **"))
        require_test_pass(success)
        for bad in ("", "** TEST SUCCEEDED **", success.replace("passed", "skipped"),
                    success.replace("Executed 1 test", "Executed 0 tests"),
                    success.replace("0 failures", "1 failures"), success + "\n** TEST FAILED **"):
            with self.assertRaises(RuntimeError):
                require_test_pass(bad)

    def test_export_requires_real_files_and_complete_observation(self):
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            root = Path(raw)
            docs, evidence = root / "Documents", root / "evidence"
            docs.mkdir()
            evidence.mkdir()
            with self.assertRaises(FileNotFoundError):
                verify_exports(docs, evidence)
            for name, data in {"cookie.txt": COOKIE_BYTES, "blob.txt": BLOB_BYTES, "keep.txt": SENTINEL_BYTES}.items():
                (docs / name).write_bytes(data)
            receipt = {"cookie": True, "blob": True, "temporaryCount": 0, "sentinel": True,
                       "closedDuringTransfer": True, "closeTimedOut": False,
                       "closedDuringExport": True, "exportFileRetained": True, "exportCloseTimedOut": False}
            (docs / "fixture-receipt.json").write_text(json.dumps(receipt))
            self.assertEqual(len(verify_exports(docs, evidence)), 3)
            for field in ("closedDuringExport", "exportFileRetained", "exportCloseTimedOut"):
                for observed in (None, not receipt[field]):
                    bad = dict(receipt)
                    if observed is None:
                        bad.pop(field)
                    else:
                        bad[field] = observed
                    (docs / "fixture-receipt.json").write_text(json.dumps(bad))
                    with self.assertRaises(RuntimeError):
                        verify_exports(docs, evidence)
            (docs / "fixture-receipt.json").write_text(json.dumps(receipt))
            (docs / "cookie.txt").write_bytes(b"different")
            with self.assertRaises(RuntimeError):
                verify_exports(docs, evidence)
            (docs / "cookie.txt").unlink()
            (docs / "cookie.txt").symlink_to(evidence / "cookie.txt")
            with self.assertRaises(RuntimeError):
                verify_exports(docs, evidence)

    def test_transfer_must_end_by_peer_before_final_bytes(self):
        event = {"path": "/slow.bin", "ended": True, "chunks": 2, "peerClosed": True, "completed": False}
        require_interrupted_transfers([event, event])
        for bad in ([], [event], [event, dict(event, peerClosed=False)],
                    [event, dict(event, completed=True, chunks=512)]):
            with self.assertRaises(RuntimeError):
                require_interrupted_transfers(bad)

    def test_real_tls_cookie_contrast_blob_and_cancelled_response(self):
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            ca, cert, key = certificate(Path(raw))
            server = FixtureServer(cert, key)
            server.start()
            context = ssl.create_default_context(cafile=str(ca))

            def get(path, cookie=None):
                client = http.client.HTTPSConnection("127.0.0.1", server.server_port, context=context, timeout=5)
                client.request("GET", path, headers={"Cookie": cookie} if cookie else {})
                response = client.getresponse()
                data = response.read()
                client.close()
                return response, data

            try:
                untrusted = http.client.HTTPSConnection("127.0.0.1", server.server_port, timeout=5)
                with self.assertRaises(ssl.SSLCertVerificationError):
                    untrusted.request("GET", "/")
                untrusted.close()
                response, _ = get("/login")
                self.assertEqual(response.status, 303)
                cookie = response.getheader("Set-Cookie").split(";", 1)[0]
                response, data = get("/cookie.txt", cookie)
                self.assertEqual((response.status, data), (200, COOKIE_BYTES))
                self.assertEqual(response.getheader("Content-Disposition"), 'attachment; filename="cookie.txt"')
                for other in (None, "fixture_session=wrong"):
                    response, data = get("/cookie.txt", other)
                    self.assertEqual(response.status, 403)
                    self.assertIn(b"Cookie denied", data)
                    self.assertIsNone(response.getheader("Content-Disposition"))
                _, home = get("/", cookie)
                self.assertIn(b"Fixture signed in", home)
                self.assertIn(b"URL.createObjectURL(new Blob", home)
                for _ in range(2):
                    client = http.client.HTTPSConnection("127.0.0.1", server.server_port, context=context, timeout=5)
                    client.request("GET", "/slow.bin", headers={"Cookie": cookie})
                    response = client.getresponse()
                    self.assertEqual(response.read(8192), b"s" * 8192)
                    response.close()
                    client.close()
                deadline = time.monotonic() + 5
                while len([e for e in server.events if e.get("ended")]) < 2 and time.monotonic() < deadline:
                    time.sleep(0.05)
                require_interrupted_transfers(server.events)
            finally:
                server.stop()


if __name__ == "__main__":
    unittest.main()
