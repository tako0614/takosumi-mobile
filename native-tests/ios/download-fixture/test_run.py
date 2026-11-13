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

from run import (certificate, owned_device, preserve_device_logs, require_interrupted_transfers,
                 require_test_pass, select_runtime, verify_exports)
from server import BLOB_BYTES, COOKIE_BYTES, SENTINEL_BYTES, FixtureServer


class HarnessTests(unittest.TestCase):
    def setUp(self):
        for name in ("UNREAPED_COMMANDS", "INTERRUPTED_COMMANDS"):
            reset = patch.object(fixture_runner, name, [])
            reset.start()
            self.addCleanup(reset.stop)

    # stage-checkpoint-controls-begin
    def test_stage_checkpoint_is_optional_atomic_and_never_a_terminal_verdict(self):
        old = {"passed": False, "cleanup": False, "simulator": {"udid": "original-id", "name": "original-name"}}
        result = {"passed": False, "cleanup": False, "source": {"fixture": "held-source-hash"},
                  "simulator": {"udid": "replacement-id", "name": "replacement-name", "bootRetry": "held-retry"}}
        device = {"udid": "replacement-id", "name": "replacement-name", "logPath": "private-unselected-path"}
        unchanged = copy.deepcopy(result)
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            out = Path(raw)
            receipt = out / "result.json"
            receipt.write_text(json.dumps(old) + "\n")
            foreign = out / ".result-checkpoint-unowned.tmp"
            foreign.write_bytes(b"unrelated sentinel")
            with patch.object(fixture_runner, "command", side_effect=AssertionError("No new CLI")) as command, \
                    patch.object(fixture_runner, "save", side_effect=AssertionError("Existing save is unchanged")) as save, \
                    patch.object(fixture_runner.time, "time", side_effect=AssertionError("No new clock")), \
                    patch.object(fixture_runner.time, "monotonic", side_effect=AssertionError("No new clock")), \
                    patch.object(fixture_runner.time, "sleep", side_effect=AssertionError("No new wait")):
                for stage in ("boot-retry-returned", "trust-returned", "before-test-returned"):
                    fixture_runner.preserve_stage_checkpoint(out, result, stage, "replacement-id", "replacement-name", device)
                    value = json.loads(receipt.read_text())
                    self.assertEqual({key: item for key, item in value.items() if key != "diagnosticCheckpoint"}, result)
                    checkpoint = value["diagnosticCheckpoint"]
                    self.assertEqual(checkpoint, {
                        "schema": "takosumi.ios-download-stage-checkpoint@v1", "diagnosticOnly": True,
                        "stage": stage, "heldSimulator": {"udid": "replacement-id", "name": "replacement-name"},
                        "createdDeviceIdentity": {"udid": "replacement-id", "name": "replacement-name"}})
                    self.assertFalse(value["passed"] or value["cleanup"])
                command.assert_not_called()
                save.assert_not_called()
            self.assertEqual(result, unchanged)
            valid = receipt.read_bytes()
            self.assertEqual({path.name for path in out.iterdir()}, {"result.json", foreign.name})
            original_temp = fixture_runner.tempfile.NamedTemporaryFile

            class PartialWrite:
                def __init__(self, stream):
                    self.stream, self.name = stream, stream.name
                def __enter__(self):
                    self.stream.__enter__()
                    return self
                def __exit__(self, *args):
                    return self.stream.__exit__(*args)
                def write(self, data):
                    self.stream.write(data[:9])
                    self.stream.flush()
                    raise OSError("private partial-write failure")

            for mode in ("serialization", "partial-write", "replace", "missing-held-identity", "unknown-stage"):
                with self.subTest(failure=mode):
                    if mode == "serialization":
                        writer = patch.object(fixture_runner.json, "dumps", side_effect=RuntimeError("private observer failure"))
                    elif mode == "partial-write":
                        writer = patch.object(fixture_runner.tempfile, "NamedTemporaryFile",
                                              side_effect=lambda **kwargs: PartialWrite(original_temp(**kwargs)))
                    elif mode == "replace":
                        writer = patch.object(fixture_runner.os, "replace", side_effect=OSError("private replace failure"))
                    else:
                        writer = patch.object(fixture_runner.tempfile, "NamedTemporaryFile",
                                              side_effect=AssertionError("Invalid metadata creates no file"))
                    with writer:
                        fixture_runner.preserve_stage_checkpoint(out, result,
                            "unknown-stage" if mode == "unknown-stage" else "trust-returned",
                            "replacement-id", "replacement-name", {} if mode == "missing-held-identity" else device)
                    self.assertEqual(receipt.read_bytes(), valid)
                    self.assertEqual(result, unchanged)
                    self.assertEqual({path.name for path in out.iterdir()}, {"result.json", foreign.name})
                    self.assertEqual(foreign.read_bytes(), b"unrelated sentinel")
            refused_unlinks = []
            def refuse_unlink(path):
                refused_unlinks.append(path)
                raise PermissionError("private temporary cleanup failure")
            with patch.object(fixture_runner.os, "replace", side_effect=OSError("private sink failure")), \
                    patch.object(Path, "unlink", autospec=True, side_effect=refuse_unlink):
                fixture_runner.preserve_stage_checkpoint(out, result, "trust-returned",
                    "replacement-id", "replacement-name", device)
            self.assertEqual(receipt.read_bytes(), valid)
            self.assertEqual(len(refused_unlinks), 1)
            temporary = refused_unlinks[0]
            self.assertEqual(temporary.parent, out)
            self.assertTrue(temporary.name.startswith(".result-checkpoint-") and temporary.name.endswith(".tmp"))
            self.assertNotEqual(temporary, foreign)
            temporary.unlink()  # Only this test's identified owned temporary.
            original = RuntimeError("original functional failure")
            raised = None
            cleanup = Mock()
            try:
                try:
                    raise original
                finally:
                    with patch.object(fixture_runner.os, "replace", side_effect=OSError("private sink failure")):
                        fixture_runner.preserve_stage_checkpoint(out, result, "trust-returned",
                            "replacement-id", "replacement-name", device)
                    cleanup()
            except Exception as error:
                raised = error
            self.assertIs(raised, original)
            cleanup.assert_called_once_with()
            self.assertEqual(receipt.read_bytes(), valid)
            self.assertEqual(result, unchanged)
            self.assertEqual({path.name for path in out.iterdir()}, {"result.json", foreign.name})
    # stage-checkpoint-controls-end

    def test_command_can_preserve_large_raw_output_without_decoding_it(self):
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            path = Path(raw) / "target-raw.log"
            argv = [sys.executable, "-c", "import sys; sys.stdout.buffer.write(b'x' * (1024 * 1024))"]
            with patch.object(Path, "read_text", side_effect=AssertionError("Raw output must not be decoded")):
                self.assertEqual(fixture_runner.command(argv, path, timeout=5, read_output=False), "")
            self.assertEqual(path.stat().st_size, 1024 * 1024)
            path = Path(raw) / "normal.log"
            self.assertEqual(fixture_runner.command([sys.executable, "-c", "print('normal stdout')"], path, timeout=5),
                             "normal stdout\n")

    def test_attachment_export_preserves_missing_or_interrupted_bundle(self):
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            out = Path(raw)
            with patch.object(fixture_runner, "command") as command:
                self.assertEqual(fixture_runner.export_test_attachments(out)["status"], "unavailable")
                (out / "Tests.xcresult").mkdir()
                fixture_runner.INTERRUPTED_COMMANDS.append({"log": str(out / "xcodebuild.log"), "reaped": True})
                self.assertEqual(fixture_runner.export_test_attachments(out)["status"], "interrupted-test")
                command.assert_not_called()

    def test_attachment_export_failure_is_diagnostic_and_original_bundle_is_preserved(self):
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            out = Path(raw)
            bundle = out / "Tests.xcresult"
            bundle.mkdir()
            sentinel = bundle / "original"
            sentinel.write_bytes(b"original failed XCTest")
            for error in (None, RuntimeError("export unavailable"), subprocess.TimeoutExpired("xcrun", 60)):
                with self.subTest(error=type(error).__name__), \
                     patch.object(fixture_runner, "command", side_effect=error) as command:
                    result = fixture_runner.export_test_attachments(out)
                    self.assertTrue(result["diagnosticOnly"])
                    self.assertEqual(result["status"], "export-failed" if error else "exported")
                    command.assert_called_once_with(
                        ["xcrun", "xcresulttool", "export", "attachments", "--path", str(bundle),
                         "--output-path", str(out / "test-attachments")],
                        out / "attachment-export.log", timeout=60)
                    self.assertEqual(sentinel.read_bytes(), b"original failed XCTest")

    def test_export_observations_retain_only_exact_owned_bounded_data(self):
        udid = "7F0B0D20-C07F-4BED-ABA8-B7C316ED4BAD"
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            root = Path(raw).resolve()
            data = root / udid / "data"
            container = data / "Containers/Data/Application/609105A8-68B2-4EB1-888D-C411B22215F5"
            docs = container / "Documents"
            docs.mkdir(parents=True)
            out = root / "evidence"
            out.mkdir()
            (out / "xcodebuild.log").write_text("original XCTest failed")
            source = docs / "fixture-export-observations.json"
            value = {"diagnosticOnly": True, "samples": [{"unixSeconds": 1794204000.0,
                     "sourceExists": True, "sourceReadable": True, "sourceMatchesOriginal": True,
                     "activityAlive": True, "activityPresented": True}],
                     "lifecycle": [{"unixSeconds": 1794203999.0, "event": "state", "state": "exporting"},
                                   {"unixSeconds": 1794203999.1, "event": "export-present-requested", "state": "exporting"},
                                   {"unixSeconds": 1794203999.2, "event": "export-present-completed", "state": "exporting"}],
                     "lifecycleTruncated": False}
            source.write_text(json.dumps(value))
            with patch.object(fixture_runner, "command", return_value=str(container)) as command:
                result = fixture_runner.preserve_export_observations({"dataPath": str(data)}, udid, out)
                self.assertEqual(result, {"diagnosticOnly": True, "status": "retained", "samples": 1})
                command.assert_called_once_with(
                    ["xcrun", "simctl", "get_app_container", udid, fixture_runner.BUNDLE, "data"],
                    out / "observation-container.log", timeout=20)
                self.assertEqual(json.loads((out / source.name).read_bytes()), value)
            retained = (out / source.name).read_bytes()
            for mode in ("extra-field", "oversized", "symlink", "hardlink", "foreign-container",
                         "unknown-event", "unknown-state", "event-extra-field", "event-overflow",
                         "event-time", "sample-type", "truncation-type"):
                with self.subTest(mode=mode):
                    source.unlink()
                    source.write_text(json.dumps(value))
                    target = container
                    if mode == "extra-field":
                        source.write_text(json.dumps({**value, "private": "must not copy"}))
                    elif mode == "oversized":
                        source.write_bytes(b" " * 16385)
                    elif mode == "symlink":
                        source.unlink()
                        source.symlink_to(out / source.name)
                    elif mode == "hardlink":
                        os.link(source, root / "alias")
                    elif mode == "foreign-container":
                        target = root / "foreign"
                    else:
                        invalid = copy.deepcopy(value)
                        if mode == "unknown-event":
                            invalid["lifecycle"][0]["event"] = "private payload must not copy"
                        elif mode == "unknown-state":
                            invalid["lifecycle"][0]["state"] = "unexpected"
                        elif mode == "event-extra-field":
                            invalid["lifecycle"][0]["url"] = "must not copy"
                        elif mode == "event-overflow":
                            invalid["lifecycle"] *= 65
                        elif mode == "event-time":
                            invalid["lifecycle"][0]["unixSeconds"] = float("nan")
                        elif mode == "sample-type":
                            invalid["samples"][0]["sourceReadable"] = 1
                        else:
                            invalid["lifecycleTruncated"] = 1
                        source.write_text(json.dumps(invalid))
                    with patch.object(fixture_runner, "command", return_value=str(target)):
                        self.assertEqual(fixture_runner.preserve_export_observations(
                            {"dataPath": str(data)}, udid, out)["status"], "unavailable")
                    self.assertEqual((out / source.name).read_bytes(), retained)
                    self.assertEqual((out / "xcodebuild.log").read_text(), "original XCTest failed")

    def test_export_observations_do_not_touch_interrupted_test_or_hide_cli_failure(self):
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            out = Path(raw)
            with patch.object(fixture_runner, "command") as command:
                self.assertEqual(fixture_runner.preserve_export_observations({}, "unused", out)["status"], "unavailable")
                (out / "xcodebuild.log").write_text("original failure")
                fixture_runner.INTERRUPTED_COMMANDS.append({"log": str(out / "xcodebuild.log"), "reaped": True})
                self.assertEqual(fixture_runner.preserve_export_observations({}, "unused", out)["status"], "interrupted-test")
                command.assert_not_called()
            fixture_runner.INTERRUPTED_COMMANDS.clear()
            udid = "7F0B0D20-C07F-4BED-ABA8-B7C316ED4BAD"
            data = out / udid / "data"
            data.mkdir(parents=True)
            with patch.object(fixture_runner, "command", side_effect=subprocess.TimeoutExpired("simctl", 20)):
                result = fixture_runner.preserve_export_observations({"dataPath": str(data.resolve())}, udid, out)
                self.assertEqual(result["errorType"], "TimeoutExpired")
                self.assertEqual(result["status"], "unavailable")
                self.assertEqual((out / "xcodebuild.log").read_text(), "original failure")

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

    def test_stalled_boot_retries_on_the_same_device_once_it_shuts_down(self):
        calls = []
        def fake_command(argv, path, timeout=60, **kwargs):
            calls.append((argv[2], Path(path).name, timeout, kwargs.get("boot_status")))
            return ""
        original = {"udid": "stalled-id", "name": "stalled-name"}
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            out = Path(raw)
            result = {"simulator": {"udid": "stalled-id", "name": "stalled-name"}}
            with patch.object(fixture_runner, "command", side_effect=fake_command):
                udid, name, live = fixture_runner.retry_boot_after_stall(
                    out, subprocess.TimeoutExpired(["xcrun", "simctl", "bootstatus", "stalled-id", "-b"], 180),
                    "stalled-id", "stalled-name", "device-type", "runtime", result, original)
            self.assertIn("first attempt", (out / "bootstatus-stalled.txt").read_text())
        self.assertEqual((udid, name), ("stalled-id", "stalled-name"))
        self.assertIs(live, original)
        self.assertEqual([call[0] for call in calls],
                         ["shutdown", "erase", "boot", "bootstatus"])
        self.assertEqual(calls[-1][2:], (180, True))
        self.assertEqual(result["simulator"]["bootRetry"], "stalled-erased-rebooted")
        self.assertNotIn("stalledUdid", result["simulator"])
    def test_stalled_boot_moves_to_a_replacement_when_shutdown_itself_fails(self):
        # A stall that wedges simctl's own shutdown cannot be erased in place
        # (run 43076506208: the 60 s shutdown bound expired behind the wedged
        # migration). The single retry must still run, on a replacement
        # device, with the corpse's identity preserved for evidence and the
        # next run's sweep.
        calls = []
        replacement = "E13F1385-F0E1-41B7-A9A9-9019B6E3866B"
        created = {}
        def fake_command(argv, path, timeout=60, **kwargs):
            calls.append((argv[2], Path(path).name, timeout, kwargs.get("boot_status")))
            if argv[2] == "shutdown":
                raise subprocess.TimeoutExpired(argv, timeout)
            if argv[2] == "create":
                created["name"] = argv[3]
                return replacement + "\n"
            if argv[2] == "list":
                return json.dumps({"devices": {"runtime": [{"udid": replacement,
                                                            "name": created["name"],
                                                            "logPath": f"/x/{replacement}"}]}})
            return ""
        original = {"udid": "stalled-id", "name": "stalled-name"}
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            out = Path(raw)
            result = {"simulator": {"udid": "stalled-id", "name": "stalled-name"}}
            with patch.object(fixture_runner, "command", side_effect=fake_command):
                udid, name, live = fixture_runner.retry_boot_after_stall(
                    out, subprocess.TimeoutExpired(["xcrun", "simctl", "bootstatus", "stalled-id", "-b"], 180),
                    "stalled-id", "stalled-name", "device-type", "runtime", result, original)
            self.assertIn("TimeoutExpired", (out / "shutdown-stalled.txt").read_text())
        self.assertEqual(udid, replacement)
        self.assertEqual(live["udid"], replacement)
        self.assertTrue(name.startswith("sumi-download-") and name != "stalled-name")
        self.assertEqual([call[0] for call in calls],
                         ["shutdown", "delete", "create", "list", "boot", "bootstatus"])
        self.assertEqual(calls[-1][2:], (180, True))
        self.assertEqual(result["simulator"]["stalledUdid"], "stalled-id")
        self.assertEqual(result["simulator"]["bootRetry"], "stalled-shutdown-failed-recreated")
        self.assertEqual(result["simulator"]["udid"], replacement)
    def test_stalled_boot_delete_failure_is_recorded_not_masked(self):
        calls = []
        replacement = "E13F1385-F0E1-41B7-A9A9-9019B6E3866B"
        created = {}
        def fake_command(argv, path, timeout=60, **kwargs):
            calls.append(argv[2])
            if argv[2] in ("shutdown", "delete"):
                raise subprocess.TimeoutExpired(argv, timeout)
            if argv[2] == "create":
                created["name"] = argv[3]
                return replacement + "\n"
            if argv[2] == "list":
                return json.dumps({"devices": {"runtime": [{"udid": replacement,
                                                            "name": created["name"]}]}})
            return ""
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            out = Path(raw)
            result = {"simulator": {"udid": "stalled-id", "name": "stalled-name"}}
            with patch.object(fixture_runner, "command", side_effect=fake_command):
                udid, _name, _live = fixture_runner.retry_boot_after_stall(
                    out, subprocess.TimeoutExpired(["xcrun", "simctl", "bootstatus", "stalled-id", "-b"], 180),
                    "stalled-id", "stalled-name", "device-type", "runtime", result, {})
            self.assertIn("TimeoutExpired", (out / "delete-stalled.txt").read_text())
        self.assertEqual(udid, replacement)
        self.assertEqual(calls, ["shutdown", "delete", "create", "list", "boot", "bootstatus"])

    def test_stalled_boot_retry_failure_retires_the_named_replacement(self):
        # If the retry fails after result.simulator was repointed at the
        # replacement, main's locals still name the corpse and its finally
        # could never reach the device the artifact names. The retry retires
        # the replacement itself so cleanup cannot claim an absent device.
        calls = []
        replacement = "E13F1385-F0E1-41B7-A9A9-9019B6E3866B"
        created = {}
        def fake_command(argv, path, timeout=60, **kwargs):
            calls.append((argv[2], Path(path).name))
            if argv[2] in ("shutdown", "bootstatus"):
                raise subprocess.TimeoutExpired(argv, timeout)
            if argv[2] == "create":
                created["name"] = argv[3]
                return replacement + "\n"
            if argv[2] == "list":
                return json.dumps({"devices": {"runtime": [{"udid": replacement,
                                                            "name": created["name"]}]}})
            return ""
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            out = Path(raw)
            result = {"simulator": {"udid": "stalled-id", "name": "stalled-name"}}
            with patch.object(fixture_runner, "command", side_effect=fake_command):
                with self.assertRaises(subprocess.TimeoutExpired):
                    fixture_runner.retry_boot_after_stall(
                        out, subprocess.TimeoutExpired(["xcrun", "simctl", "bootstatus", "stalled-id", "-b"], 180),
                        "stalled-id", "stalled-name", "device-type", "runtime", result, {})
        self.assertEqual([call[0] for call in calls],
                         ["shutdown", "delete", "create", "list", "boot", "bootstatus", "delete"])
        self.assertEqual(result["simulator"]["udid"], replacement)
        self.assertTrue(result["simulator"]["replacementDeleted"])
        self.assertNotIn("bootRetry", result["simulator"])
    def test_stalled_boot_retry_failure_records_owed_cleanup(self):
        # A replacement that cannot even be deleted is recorded as owed
        # cleanup instead of being silently leaked or claimed removed.
        calls = []
        replacement = "E13F1385-F0E1-41B7-A9A9-9019B6E3866B"
        created = {}
        def fake_command(argv, path, timeout=60, **kwargs):
            calls.append((argv[2], Path(path).name))
            if argv[2] == "shutdown" or argv[2] == "bootstatus":
                raise subprocess.TimeoutExpired(argv, timeout)
            if argv[2] == "delete" and Path(path).name == "delete-replacement.log":
                raise subprocess.TimeoutExpired(argv, timeout)
            if argv[2] == "create":
                created["name"] = argv[3]
                return replacement + "\n"
            if argv[2] == "list":
                return json.dumps({"devices": {"runtime": [{"udid": replacement,
                                                            "name": created["name"]}]}})
            return ""
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            out = Path(raw)
            result = {"simulator": {"udid": "stalled-id", "name": "stalled-name"}}
            with patch.object(fixture_runner, "command", side_effect=fake_command):
                with self.assertRaises(subprocess.TimeoutExpired):
                    fixture_runner.retry_boot_after_stall(
                        out, subprocess.TimeoutExpired(["xcrun", "simctl", "bootstatus", "stalled-id", "-b"], 180),
                        "stalled-id", "stalled-name", "device-type", "runtime", result, {})
        self.assertIn("replacementCleanupError", result["simulator"])
        self.assertNotIn("replacementDeleted", result["simulator"])

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

    def test_trust_timeout_snapshot_precedes_retirement_and_cannot_mask_failure(self):
        argv = ["xcrun", "simctl", "keychain", "owned-id", "add-root-cert", "owned-ca.pem"]
        for snapshot_failure, write_failure in ((False, False), (True, False), (False, True)):
            with self.subTest(snapshot_failure=snapshot_failure, write_failure=write_failure), \
                 tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
                original = subprocess.TimeoutExpired(argv, 60)
                process = Mock(pid=424242)
                process.wait.side_effect = [original, -15]
                process.poll.return_value = None
                path = Path(raw) / "trust.log"
                order = []

                def snapshot(pid):
                    self.assertEqual(pid, process.pid)
                    process.poll.assert_not_called()
                    process.terminate.assert_not_called()
                    order.append("snapshot")
                    if snapshot_failure:
                        raise RuntimeError("diagnostic unavailable")
                    return {"pid": pid, "birthMicros": 123, "imageClass": "simctl"}

                original_save = fixture_runner.save

                def save(path, data):
                    if path.name == "trust.stage.json":
                        process.terminate.assert_called_once_with()
                        order.append("save")
                        if write_failure:
                            raise OSError("diagnostic write unavailable")
                    original_save(path, data)

                with patch.object(fixture_runner.subprocess, "Popen", return_value=process), \
                     patch.object(fixture_runner, "owned_cli_snapshot", side_effect=snapshot), \
                     patch.object(fixture_runner, "save", side_effect=save):
                    with self.assertRaises(subprocess.TimeoutExpired) as caught:
                        fixture_runner.command(argv, path, trust_trace=True)
                self.assertIs(caught.exception, original)
                self.assertEqual(order, ["snapshot", "save"])
                self.assertEqual([call.kwargs for call in process.wait.call_args_list],
                                 [{"timeout": 60}, {"timeout": 5}])
                self.assertTrue(json.loads(path.with_suffix(".process.json").read_text())["reaped"])
                if not write_failure:
                    stage = json.loads(path.with_suffix(".stage.json").read_text())
                    self.assertEqual(stage["errorType"], "TimeoutExpired")
                    self.assertNotIn("exit", stage)
                    self.assertEqual("snapshotErrorType" in stage, snapshot_failure)

    def test_trust_success_has_exit_receipt_without_failure_snapshot(self):
        argv = ["xcrun", "simctl", "keychain", "owned-id", "add-root-cert", "owned-ca.pem"]
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            path = Path(raw) / "trust.log"
            process = Mock(pid=424242)
            process.wait.return_value = 0
            with patch.object(fixture_runner.subprocess, "Popen", return_value=process), \
                 patch.object(fixture_runner, "owned_cli_snapshot") as snapshot:
                fixture_runner.command(argv, path, trust_trace=True)
            snapshot.assert_not_called()
            process.terminate.assert_not_called()
            stage = json.loads(path.with_suffix(".stage.json").read_text())
            self.assertEqual(stage["exit"], 0)
            self.assertEqual(stage["configuredWaitSeconds"], 60)
            self.assertTrue(stage["diagnosticOnly"])
            self.assertGreaterEqual(stage["stageElapsedSeconds"], 0)

    def test_owned_snapshot_rejects_changed_birth_or_foreign_parent(self):
        ctypes = fixture_runner.ctypes
        for mode in ("simctl", "other", "birth-changed", "foreign-parent"):
            with self.subTest(mode=mode):
                lib = Mock()
                reads = []

                def facts(pid, flavor, arg, target, size):
                    info = fixture_runner.OwnedBsdInfo()
                    info.pid, info.parent, info.status = pid, os.getpid(), 2
                    info.seconds, info.micros = 100, 3
                    if mode == "foreign-parent":
                        info.parent += 1
                    if mode == "birth-changed" and reads:
                        info.micros += 1
                    reads.append(pid)
                    self.assertEqual((flavor, arg, size), (3, 0, 136))
                    ctypes.memmove(target, ctypes.byref(info), size)
                    return size

                def image(pid, target, size):
                    value = b"/private/fixture/simctl" if mode != "other" else b"/private/hidden-name"
                    self.assertEqual(size, 4096)
                    ctypes.memmove(target, value + b"\0", len(value) + 1)
                    return len(value)

                lib.proc_pidinfo.side_effect, lib.proc_pidpath.side_effect = facts, image
                with patch.object(fixture_runner.platform, "system", return_value="Darwin"), \
                     patch.object(ctypes, "CDLL", return_value=lib):
                    if mode in ("birth-changed", "foreign-parent"):
                        with self.assertRaises(RuntimeError):
                            fixture_runner.owned_cli_snapshot(424242)
                    else:
                        result = fixture_runner.owned_cli_snapshot(424242)
                        self.assertEqual(result["birthMicros"], 100000003)
                        self.assertEqual(result["imageClass"], mode)
                        self.assertNotIn("private", json.dumps(result))
                if mode == "foreign-parent":
                    lib.proc_pidpath.assert_not_called()

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
            for log, recent, active in (
                                 # Run 42834538144: the migrator the OS still
                                 # reports running is in flight, not stalled —
                                 # it can use the same 300-second ceiling.
                                 (entry(5, "launch") + entry(175, "launch"), False, True),
                                 (entry(5, "launch") + entry(20, "keys") + entry(175, "keys"), False, True),
                                 (entry(5, "launch") + entry(165, "safari") + entry(175, "safari"), True, True),
                                 # Run 42739950403: a deadline read behind
                                 # simctl's buffered output still shows a
                                 # running migrator seconds old — the floor
                                 # must not refuse the bounded continuation.
                                 (entry(33, "launch") + entry(117, "keys") + entry(129, "safari") +
                                  entry(142, "safari"), True, True),
                                 # Run 42672706585: migration ended and the boot
                                 # reached Waiting on System App; that stage
                                 # transition is the same forward motion.
                                 ("Status=2, isTerminal=NO, Elapsed=02:00.\n"
                                  "Reason:Running plugin keys (a.migrator)\n"
                                  "Status=4, isTerminal=NO, Elapsed=02:58.\n"
                                  "\tWaiting on System App\n", True, False),
                                 # A boot stuck inside the same post-migration
                                 # stage still produces no recent transition.
                                 ("Status=2, isTerminal=NO, Elapsed=00:05.\n"
                                  "Reason:Running plugin keys (a.migrator)\n"
                                  "Status=4, isTerminal=NO, Elapsed=01:00.\n"
                                  "\tWaiting on System App\n"
                                  "Status=4, isTerminal=NO, Elapsed=02:55.\n"
                                  "\tWaiting on System App\n", False, False),
                                 ("Status=4, isTerminal=NO, Elapsed=02:55.\n", False, False),
                                 ("", False, False)):
                path.write_text(log)
                proof = fixture_runner.migration_progress(path)
                self.assertEqual(proof["recentMigrationProgress"], recent)
                self.assertEqual(proof["activeMigration"], active)

    def test_cold_boot_continues_while_one_named_migrator_is_still_running(self):
        # Run 42834538144: both attempts died inside a still-running
        # 00LaunchServicesMigrator. Its in-flight state earns the same bounded
        # continuation once; a dead boot still fails at the 300-second cap.
        argv = ["xcrun", "simctl", "bootstatus", "owned-id", "-b"]
        log = (b"Status=2, isTerminal=NO, Elapsed=00:09.\nReason:Running plugin launch (a.migrator)\n"
               b"Status=2, isTerminal=NO, Elapsed=02:58.\nReason:Running plugin launch (a.migrator)\n")
        process = Mock(pid=424242)
        original = subprocess.TimeoutExpired(argv, 180)
        process.wait.side_effect = [original, 0]
        process.poll.return_value = None

        def spawn(*_, **kwargs):
            kwargs["stdout"].write(log)
            kwargs["stdout"].flush()
            return process

        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            path = Path(raw) / "bootstatus.log"
            with patch.object(fixture_runner.subprocess, "Popen", side_effect=spawn):
                self.assertEqual(fixture_runner.command(argv, path, timeout=180, boot_status=True), log.decode())
            proof = json.loads(path.with_suffix(".progress.json").read_text())
            self.assertEqual(proof["continuationReason"], "active-migration")
            self.assertEqual(proof["extensionSeconds"], 120)
            self.assertEqual(proof["continuationExit"], 0)

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
        cases = ((waiting + finished, 0, True),
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

    def test_native_finished_live_cli_must_exit_zero_within_total_boot_budget(self):
        argv = ["xcrun", "simctl", "bootstatus", "owned-id", "-b"]
        # 6d0a's real log/progress observed native Finished at196s while poll
        # still returned None; retirement then observed exit0 without signals.
        log = (b"Status=4, isTerminal=NO, Elapsed=03:15.\n\tWaiting on System App\n"
               b"Status=4294967295, isTerminal=YES, Elapsed=03:16.\n\tFinished\n")
        for final, observed_at in ((0, 197), (1, 197), (subprocess.TimeoutExpired(argv, 104), 197), (0, 301)):
            with self.subTest(final=final, observed_at=observed_at):
                original = subprocess.TimeoutExpired(argv, 180)
                process = Mock(pid=424242)
                process.wait.side_effect = [original, final, -15]
                process.poll.side_effect = [None, final if isinstance(final, int) else None]

                def spawn(*_, **kwargs):
                    kwargs["stdout"].write(log)
                    kwargs["stdout"].flush()
                    return process

                with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
                    path = Path(raw) / "bootstatus.log"
                    with patch.object(fixture_runner.subprocess, "Popen", side_effect=spawn) as launch, \
                         patch.object(fixture_runner.time, "monotonic", side_effect=[0, 196, 196, observed_at]):
                        if final == 0 and observed_at <= 300:
                            self.assertEqual(fixture_runner.command(argv, path, timeout=180, boot_status=True), log.decode())
                            process.terminate.assert_not_called()
                            process.kill.assert_not_called()
                            self.assertFalse(path.with_suffix(".process.json").exists())
                        else:
                            with self.assertRaises(subprocess.TimeoutExpired) as caught:
                                fixture_runner.command(argv, path, timeout=180, boot_status=True)
                            self.assertIs(caught.exception, original)
                        launch.assert_called_once()
                        self.assertEqual([call.kwargs["timeout"] for call in process.wait.call_args_list[:2]], [180, 104])
                        proof = json.loads(path.with_suffix(".progress.json").read_text())
                        self.assertEqual(proof["continuationReason"], "native-finished-cli-exit")
                        self.assertEqual(proof["originalDeadlineSeconds"], 180)
                        self.assertEqual(proof["extensionSeconds"], 104)

    def test_boot_diagnostics_do_not_extend_the_total_readiness_budget(self):
        argv = ["xcrun", "simctl", "bootstatus", "owned-id", "-b"]
        for exit_code, delayed_write in ((0, 1), (None, 2)):
            with self.subTest(exit_code=exit_code, delayed_write=delayed_write):
                original = subprocess.TimeoutExpired(argv, 180)
                process = Mock(pid=424242)
                process.wait.side_effect = original
                process.poll.side_effect = [exit_code, 0]
                clock = [0]
                writes = [0]
                save = fixture_runner.save

                def spawn(*_, **kwargs):
                    kwargs["stdout"].write(b"Status=4294967295, isTerminal=YES, Elapsed=03:16.\nFinished\n")
                    kwargs["stdout"].flush()
                    return process

                def slow_diagnostic(path, value):
                    save(path, value)
                    if path.name.endswith(".progress.json"):
                        writes[0] += 1
                        clock[0] = 301 if writes[0] >= delayed_write else 196

                with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
                    path = Path(raw) / "bootstatus.log"
                    with patch.object(fixture_runner.subprocess, "Popen", side_effect=spawn) as launch, \
                         patch.object(fixture_runner.time, "monotonic", side_effect=lambda: clock[0]), \
                         patch.object(fixture_runner, "save", side_effect=slow_diagnostic):
                        with self.assertRaises(subprocess.TimeoutExpired) as caught:
                            fixture_runner.command(argv, path, timeout=180, boot_status=True)
                        self.assertIs(caught.exception, original)
                        launch.assert_called_once()
                        process.wait.assert_called_once_with(timeout=180)
                        process.terminate.assert_not_called()
                        process.kill.assert_not_called()
                        proof = json.loads(path.with_suffix(".progress.json").read_text())
                        self.assertEqual(proof["remainingBootSeconds"], 0)

    def test_continuation_evidence_failure_preserves_original_timeout_and_reaps_child(self):
        argv = ["xcrun", "simctl", "bootstatus", "owned-id", "-b"]
        original = subprocess.TimeoutExpired(argv, 180)
        process = Mock(pid=424242)
        process.wait.side_effect = [original, -15]
        process.poll.return_value = None
        save = fixture_runner.save
        writes = [0]

        def spawn(*_, **kwargs):
            kwargs["stdout"].write(b"Status=4294967295, isTerminal=YES, Elapsed=03:16.\nFinished\n")
            kwargs["stdout"].flush()
            return process

        def fail_continuation_write(path, value):
            if path.name.endswith(".progress.json"):
                writes[0] += 1
                if writes[0] == 2:
                    raise OSError("fixture progress write failure")
            save(path, value)

        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            path = Path(raw) / "bootstatus.log"
            with patch.object(fixture_runner.subprocess, "Popen", side_effect=spawn) as launch, \
                 patch.object(fixture_runner, "save", side_effect=fail_continuation_write):
                with self.assertRaises(subprocess.TimeoutExpired) as caught:
                    fixture_runner.command(argv, path, timeout=180, boot_status=True)
                self.assertIs(caught.exception, original)
                self.assertTrue(any("progress" in note and "OSError" in note for note in original.__notes__))
                launch.assert_called_once()
                process.terminate.assert_called_once_with()
                self.assertEqual([call.kwargs["timeout"] for call in process.wait.call_args_list], [180, 5])
                self.assertTrue(json.loads(path.with_suffix(".process.json").read_text())["reaped"])

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

    def test_only_owned_device_text_logs_are_kept_with_strict_bounds(self):
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            root = Path(raw)
            udid = "22F3E5C2-C70D-4096-B876-C4F3B9EBAD14"
            logs = root / udid
            logs.mkdir()
            for index in range(5):
                (logs / f"system{index}.log").write_bytes(b"x" * 70000 + bytes([index]))
            (logs / "crash.dmp").write_bytes(b"not text evidence")
            result = preserve_device_logs({"logPath": str(logs)}, udid, root / "evidence", root)
            self.assertEqual(len(result["files"]), 4)
            self.assertTrue(result["filesTruncated"])
            self.assertFalse(result["entriesTruncated"])
            self.assertTrue(result["diagnosticOnly"])
            for index, entry in enumerate(result["files"]):
                self.assertEqual((entry["sourceBytes"], entry["keptBytes"], entry["truncated"]), (70001, 65536, True))
                self.assertEqual((root / "evidence" / entry["name"]).read_bytes(), b"x" * 65535 + bytes([index]))
            self.assertFalse((root / "evidence/crash.dmp").exists())

    def test_device_logs_refuse_foreign_paths_and_links(self):
        with tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR", "/root/dev/tmp")) as raw:
            root = Path(raw)
            udid = "22F3E5C2-C70D-4096-B876-C4F3B9EBAD14"
            foreign = root / "foreign"
            foreign.mkdir()
            (foreign / "system.log").write_text("foreign private log")
            for value in (None, "", [], False):
                result = preserve_device_logs({"logPath": value}, udid, root / "invalid", root)
                self.assertEqual(result["errorType"], "ValueError")
                self.assertFalse((root / "invalid").exists())
            for index, path in enumerate((foreign, Path(udid), root / ".." / udid)):
                result = preserve_device_logs({"logPath": str(path)}, udid, root / f"out-{index}", root)
                self.assertEqual(result["errorType"], "ValueError")
                self.assertFalse((root / f"out-{index}").exists())
            selected = root / udid
            selected.symlink_to(foreign, target_is_directory=True)
            result = preserve_device_logs({"logPath": str(selected)}, udid, root / "alias", root)
            self.assertEqual(result["errorType"], "ValueError")
            self.assertFalse((root / "alias").exists())
            selected.unlink()
            selected.mkdir()
            (selected / "symlink.log").symlink_to(foreign / "system.log")
            os.link(foreign / "system.log", selected / "hardlink.log")
            result = preserve_device_logs({"logPath": str(selected)}, udid, root / "links", root)
            self.assertEqual(len(result["files"]), 2)
            self.assertTrue(all(entry["errorType"] == "ValueError" for entry in result["files"]))
            self.assertEqual(list((root / "links").iterdir()), [])

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
