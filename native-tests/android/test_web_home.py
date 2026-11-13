"""Android-SDK-free controls; label matching uses an existing JDK, never the App."""
import hashlib
import io
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch
import xml.etree.ElementTree as ET
import zipfile

import run_web_home as runner
from fixture_network import ANDROID, CONFIG, Overlay, certificate_bytes, verify_network, domain_text_from_xmltree

CERT = b"-----BEGIN CERTIFICATE-----\nc3ludGhldGljLXB1YmxpYy1maXh0dXJl\n-----END CERTIFICATE-----\n"
TABLE = """Package name=com.takosumi.fixture.android id=7f
 resource 0x7f120003 xml/sumi_fixture_network_security
   () (file) res/xml/sumi_fixture_network_security.xml type=XML
 resource 0x7f130001 raw/sumi_fixture_ca
   () (file) res/raw/sumi_fixture_ca.pem
"""
XMLTREE = """E: network-security-config (line=2)
    E: domain-config (line=3)
      A: cleartextTrafficPermitted="false" (Raw: "false")
        E: domain (line=4)
          A: includeSubdomains="false" (Raw: "false")
            T: '127.0.0.1'
        E: domain (line=5)
          A: includeSubdomains="false" (Raw: "false")
            T: 'localhost'
        E: trust-anchors (line=6)
            E: certificates (line=6)
              A: src=@0x7f130001
"""


class Controls(unittest.TestCase):
    def setUp(self):
        parent = Path(os.environ.get("TMPDIR", "/root/dev/tmp"))
        self.root = Path(self.enterContext(tempfile.TemporaryDirectory(dir=parent)))
        self.cert = self.root / "ca.pem"
        self.cert.write_bytes(CERT)

    def project(self):
        generated = self.root / "src-tauri/gen/android"
        main = generated / "app/src/main"
        (main / "res/xml").mkdir(parents=True)
        (generated / "app/build.gradle.kts").write_text('applicationId = "com.takosumi.fixture.android"')
        manifest = main / "AndroidManifest.xml"
        manifest.write_text('<manifest xmlns:android="http://schemas.android.com/apk/res/android">'
                            '<application android:allowBackup="false" /></manifest>')
        return generated, main, manifest

    def log(self):
        values = f"class={runner.CLASS}\ntest={runner.METHOD}\nnumtests=1\ncurrent=1".splitlines()
        block = "\n".join("INSTRUMENTATION_STATUS: " + value for value in values)
        return block + "\nINSTRUMENTATION_STATUS_CODE: 1\n" + block + (
            "\nINSTRUMENTATION_STATUS_CODE: 0\nOK (1 test)\nINSTRUMENTATION_CODE: -1\n")

    def result(self):
        return {"passed": True, "cases": list(runner.CASES), "diagnosticsSaved": True,
                "oldPid": "123", "newPid": "456", "beforeColdDocument": "Document 11111111-1111-4111-8111-111111111111",
                "afterColdDocument": "Document 22222222-2222-4222-8222-222222222222"}

    def test_one_exact_nonempty_case_required_without_skip_failure_or_retry(self):
        log, result = self.log(), self.result()
        self.assertEqual(runner.require_ui_pass(log, result)["tests"], 1)
        for changed in ("", log + log, log.replace("CODE: 0", "CODE: -3"),
                        log.replace(runner.METHOD, "other"), log.replace("OK (1 test)", "OK (0 tests)"),
                        log.replace("numtests=1", "numtests=2"), log + "INSTRUMENTATION_FAILED: aborted\n"):
            with self.subTest(log=changed), self.assertRaises(RuntimeError):
                runner.require_ui_pass(changed, result)

    def test_actual_driver_connection_label_matcher_rejects_foreign_rows(self):
        compiler, java = shutil.which("javac"), shutil.which("java")
        self.assertIsNotNone(compiler, "Existing JDK17+ required for the driver label control")
        self.assertIsNotNone(java, "Existing Java runtime required for the driver label control")
        classes = self.root / "label-classes"
        classes.mkdir()
        helper = runner.HERE / "web-home-fixture/src/androidTest/java/com/takosumi/fixture/webhome/ConnectionLabel.java"
        control = runner.HERE / "label-controls/ConnectionLabelTest.java"
        generic = runner.HERE / "web-home-fixture/src/androidTest/java/com/takosumi/fixture/webhome/AccessibleName.java"
        built = subprocess.run([compiler, "--release", "17", "-encoding", "UTF-8", "-d", str(classes),
                                str(generic), str(helper), str(control)], capture_output=True, text=True, timeout=15)
        self.assertEqual(built.returncode, 0, built.stderr)
        checked = subprocess.run([java, "-cp", str(classes), "com.takosumi.fixture.webhome.ConnectionLabelTest"],
                                 capture_output=True, text=True, timeout=10)
        self.assertEqual(checked.returncode, 0, checked.stderr)
        self.assertRegex(checked.stdout, r"PASS: saved connection label checks=[1-9][0-9]*")

    def test_actual_driver_exact_accessible_name_rejects_ambiguous_foreign_or_hidden_actions(self):
        compiler, java = shutil.which("javac"), shutil.which("java")
        self.assertIsNotNone(compiler, "Existing JDK17+ required for the accessible-name control")
        self.assertIsNotNone(java, "Existing Java runtime required for the accessible-name control")
        classes = self.root / "accessible-name-classes"
        classes.mkdir()
        helper = runner.HERE / "web-home-fixture/src/androidTest/java/com/takosumi/fixture/webhome/AccessibleName.java"
        control = runner.HERE / "label-controls/AccessibleNameTest.java"
        built = subprocess.run([compiler, "--release", "17", "-encoding", "UTF-8", "-d", str(classes),
                                str(helper), str(control)], capture_output=True, text=True, timeout=15)
        self.assertEqual(built.returncode, 0, built.stderr)
        checked = subprocess.run([java, "-cp", str(classes), "com.takosumi.fixture.webhome.AccessibleNameTest"],
                                 capture_output=True, text=True, timeout=10)
        self.assertEqual(checked.returncode, 0, checked.stderr)
        self.assertRegex(checked.stdout, r"PASS: exact accessible name checks=[1-9][0-9]*")

    def test_actual_driver_save_wait_requires_observed_ime_and_one_back(self):
        compiler, java = shutil.which("javac"), shutil.which("java")
        self.assertIsNotNone(compiler, "Existing JDK17+ required for the driver keyboard control")
        self.assertIsNotNone(java, "Existing Java runtime required for the driver keyboard control")
        classes = self.root / "keyboard-classes"
        classes.mkdir()
        helper = runner.HERE / "web-home-fixture/src/androidTest/java/com/takosumi/fixture/webhome/ConnectionSaveKeyboard.java"
        control = runner.HERE / "label-controls/ConnectionSaveKeyboardTest.java"
        built = subprocess.run([compiler, "--release", "17", "-encoding", "UTF-8", "-d", str(classes),
                                str(helper), str(control)], capture_output=True, text=True, timeout=15)
        self.assertEqual(built.returncode, 0, built.stderr)
        checked = subprocess.run([java, "-cp", str(classes), "com.takosumi.fixture.webhome.ConnectionSaveKeyboardTest"],
                                 capture_output=True, text=True, timeout=10)
        self.assertEqual(checked.returncode, 0, checked.stderr)
        self.assertRegex(checked.stdout, r"PASS: connection Save keyboard checks=[1-9][0-9]*")

    def test_actual_driver_back_wait_metadata_is_bounded_and_never_a_ready_oracle(self):
        compiler, java = shutil.which("javac"), shutil.which("java")
        self.assertIsNotNone(compiler, "Existing JDK17+ required for the back wait metadata control")
        self.assertIsNotNone(java, "Existing Java runtime required for the back wait metadata control")
        classes = self.root / "back-wait-classes"
        classes.mkdir()
        helper = runner.HERE / "web-home-fixture/src/androidTest/java/com/takosumi/fixture/webhome/BackWaitObservation.java"
        control = runner.HERE / "label-controls/BackWaitObservationTest.java"
        built = subprocess.run([compiler, "--release", "17", "-encoding", "UTF-8", "-d", str(classes),
                                str(helper), str(control)], capture_output=True, text=True, timeout=15)
        self.assertEqual(built.returncode, 0, built.stderr)
        checked = subprocess.run([java, "-cp", str(classes), "com.takosumi.fixture.webhome.BackWaitObservationTest"],
                                 capture_output=True, text=True, timeout=10)
        self.assertEqual(checked.returncode, 0, checked.stderr)
        self.assertRegex(checked.stdout, r"PASS: back wait metadata checks=[1-9][0-9]*")

    def test_app_screen_observation_helper_is_debug_only_bounded_and_failure_neutral(self):
        compiler, java = shutil.which("javac"), shutil.which("java")
        self.assertIsNotNone(compiler, "Existing JDK17+ required for the native observation helper control")
        self.assertIsNotNone(java, "Existing Java runtime required for the native observation helper control")
        classes = self.root / "native-observation-classes"
        classes.mkdir()
        helper = runner.ROOT / "src-tauri/plugins/app-screen/android/src/main/java/AppScreenObservation.java"
        control = runner.HERE / "label-controls/AppScreenObservationTest.java"
        built = subprocess.run([compiler, "--release", "17", "-encoding", "UTF-8", "-d", str(classes),
                                str(helper), str(control)], capture_output=True, text=True, timeout=15)
        self.assertEqual(built.returncode, 0, built.stderr)
        checked = subprocess.run([java, "-cp", str(classes), "com.takosumi.appscreen.AppScreenObservationTest"],
                                 capture_output=True, text=True, timeout=10)
        self.assertEqual(checked.returncode, 0, checked.stderr)
        self.assertRegex(checked.stdout, r"PASS: native observation controls checks=[1-9][0-9]*")

    def test_ui_result_requires_all_operations_and_actual_cold_replacement(self):
        for defect in ("no-pass", "missing-operation", "same-pid", "invalid-pid", "same-document", "no-diagnostics"):
            result = self.result()
            if defect == "no-pass": result["passed"] = False
            if defect == "missing-operation": result["cases"].pop()
            if defect == "same-pid": result["newPid"] = result["oldPid"]
            if defect == "invalid-pid": result["oldPid"] = "123 456"
            if defect == "same-document": result["afterColdDocument"] = result["beforeColdDocument"]
            if defect == "no-diagnostics": result["diagnosticsSaved"] = False
            with self.subTest(defect=defect), self.assertRaises(RuntimeError):
                runner.require_ui_pass(self.log(), result)

    def test_kernel_drain_and_normal_exit_are_separate_requirements(self):
        value = {"complete": True, "kernelNoChildren": True, "remaining": [], "error": None,
                 "cleanupErrors": [], "nonFatalErrors": [], "rootExit": 0, "forcedCleanup": 0}
        runner.receipt(value, normal=True)
        runner.receipt(dict(value, rootExit=-15, forcedCleanup=2), normal=False)
        for key, changed in (("complete", False), ("kernelNoChildren", False), ("remaining", [123]),
                             ("cleanupErrors", ["error"]), ("error", "error"), ("nonFatalErrors", ["error"]),
                             ("rootExit", -15), ("forcedCleanup", 1)):
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                runner.receipt(dict(value, **{key: changed}), normal=True)

        # stop-observation-controls-begin (existing owning method)
        diagnostic = {"schema": "takosumi.guard-stop-observation@v1", "diagnosticOnly": True,
                      "capacity": 64, "truncated": False, "records": []}
        runner.receipt(dict(value, stopObservation=diagnostic), normal=True)
        with self.assertRaises(RuntimeError):
            runner.receipt(dict(value, forcedCleanup=1, stopObservation=diagnostic), normal=True)

        class RefusingProof(dict):
            def __setitem__(self, key, item):
                raise OSError("private observation sink failure")

        for mode in ("normal", "already-exited", "kill-timeout", "wait-timeout", "signal-error", "wait10-error", "observer", "sink"):
            with self.subTest(stop=mode):
                emulator = runner.Emulator.__new__(runner.Emulator)
                emulator.process = Mock(pid=101)
                emulator.process.poll.return_value = 0 if mode == "already-exited" else None
                emulator.birth, emulator.fd = 42, 909
                emulator.output = Mock()
                emulator.proof = self.root / "guard.json"
                emulator.proof.write_text(runner.json.dumps(dict(value, stopObservation=diagnostic)))
                emulator.run = Mock(return_value="ACK private output")
                original = PermissionError(1, "private signal failure")
                kill_timeout = subprocess.TimeoutExpired(["private command"], 15)
                wait_timeout = subprocess.TimeoutExpired(["private wait"], 15)
                wait10_error = subprocess.TimeoutExpired(["private wait"], 10)
                fallback = mode in ("kill-timeout", "wait-timeout", "signal-error", "wait10-error", "observer", "sink")
                if fallback and mode != "wait-timeout":
                    emulator.run.side_effect = kill_timeout
                emulator.process.wait.side_effect = ([wait_timeout, 0] if mode == "wait-timeout" else
                    [wait10_error] if mode == "wait10-error" else [0])
                raised = None
                proof = RefusingProof() if mode == "sink" else {}
                with patch.object(runner.signal, "pidfd_send_signal",
                                  side_effect=original if mode in ("signal-error", "observer", "sink") else None) as sent, \
                        patch.object(runner.os, "close") as closed:
                    if mode == "observer":
                        observation = patch.object(runner, "update_stop_observation", side_effect=RuntimeError("private observer failure"))
                    else:
                        observation = patch.object(runner, "update_stop_observation", wraps=runner.update_stop_observation)
                    with observation:
                        try:
                            returned = emulator.stop()
                        except Exception as error:
                            raised = error
                        finally:
                            runner.preserve_stop_observation(emulator, proof)
                    emulator.process.poll.assert_called_once_with()
                    emulator.output.close.assert_called_once_with()
                    closed.assert_called_once_with(909)
                    self.assertIsNone(emulator.fd)
                    self.assertEqual(sent.call_count, int(fallback))
                    if fallback:
                        sent.assert_called_once_with(909, runner.signal.SIGTERM)
                self.assertEqual(emulator.run.call_count, int(mode != "already-exited"))
                if mode != "already-exited":
                    emulator.run.assert_called_once_with("emu", "kill", timeout=15)
                expected_waits = ([] if mode in ("already-exited", "signal-error", "observer", "sink") else
                                  [15, 10] if mode == "wait-timeout" else
                                  [10] if fallback else [15])
                self.assertEqual([call.kwargs["timeout"] for call in emulator.process.wait.call_args_list], expected_waits)
                if mode in ("signal-error", "observer", "sink"):
                    self.assertIs(raised, original)
                elif mode == "wait10-error":
                    self.assertIs(raised, wait10_error)
                else:
                    self.assertIsNone(raised)
                    self.assertEqual(returned, dict(value, stopObservation=diagnostic))
                if mode in ("observer", "sink"):
                    self.assertNotIn("emulatorStopObservation", proof)
                else:
                    trace = proof["emulatorStopObservation"]
                    self.assertEqual((trace["guardPid"], trace["guardStartTick"]), (101, 42))
                    self.assertNotIn("private", runner.json.dumps(trace))
                    stages = [event["stage"] for event in trace["records"]]
                    self.assertEqual("fallback" in stages, fallback)
                    self.assertEqual("pidfd_sigterm_returned" in stages, fallback and mode != "signal-error")
                    if fallback:
                        branch = next(event for event in trace["records"] if event["stage"] == "fallback")
                        self.assertEqual(branch["phase"], "guard_wait_15" if mode == "wait-timeout" else "emu_kill")
                        self.assertEqual(branch["errorCategory"], "command_timeout")
                    self.assertEqual(stages[-2:], ["output_closed", "pidfd_closed"])
                    self.assertFalse(trace["truncated"])

        absent = runner.Emulator.__new__(runner.Emulator)
        absent.process = None
        self.assertIsNone(absent.stop())
        self.assertEqual([event["stage"] for event in absent.stop_observation["records"]], ["start", "no_process"])
        self.assertIsNone(absent.stop_observation["guardPid"])
        for _ in range(20):
            runner.observe_stop(absent, "guard_poll_returned", exit_code=0)
        self.assertEqual(len(absent.stop_observation["records"]), runner.STOP_OBSERVATION_CAP)
        self.assertTrue(absent.stop_observation["truncated"])
        # stop-observation-controls-end

    def test_failed_guest_anr_reads_are_owned_bounded_and_keep_raw_output_outside_evidence(self):
        scratch, out = self.root / "scratch", self.root / "evidence"
        scratch.mkdir(); out.mkdir()
        emulator = Mock(adb=Path("/sdk/platform-tools/adb"))
        calls = []
        def command(argv, path, **options):
            calls.append((argv, path, options))
            path.write_bytes(b"ANR in com.android.systemui\n" + b"x" * (1024 * 1024))
            return ""
        with patch.object(runner.runtime, "command", side_effect=command):
            result = runner.preserve_guest_anr(emulator, scratch, out)
        self.assertTrue(result["diagnosticOnly"])
        self.assertEqual(emulator.owned.call_count, 7)
        self.assertEqual([argv for argv, _, _ in calls], [
            [str(emulator.adb), "-s", runner.SERIAL, "shell", *query]
            for query in (("dumpsys", "activity", "lastanr"),
                          ("dumpsys", "activity", "lastanr-traces"),
                          ("logcat", "-b", "events", "-d", "-v", "threadtime,epoch,usec", "-s", "am_anr"),
                          ("dumpsys", "dropbox", "--print", "system_app_anr"),
                          ("dumpsys", "activity", "activities"),
                          ("dumpsys", "window", "windows"),
                          ("logcat", "-b", "crash", "-d", "-v", "threadtime,epoch,usec"))])
        for _, raw, options in calls:
            self.assertEqual(raw.parent, scratch)
            self.assertEqual(options, {"timeout": 10, "read_output": False})
        for name, entry in result["queries"].items():
            saved = out / (name + ".log")
            self.assertTrue(entry["truncated"])
            self.assertGreater(entry["sourceBytes"], entry["keptBytes"])
            self.assertEqual(saved.stat().st_size, entry["keptBytes"])
            self.assertEqual(entry["keptBytes"],
                             (512 if name in ("guest-last-anr-traces", "guest-system-app-anr") else 128) * 1024)
            self.assertEqual(hashlib.sha256(saved.read_bytes()).hexdigest(), entry["sha256"])
        self.assertFalse(any(out.glob("*-raw.log")))
        self.assertNotIn("passed", result)

    def test_guest_ownership_loss_before_anr_events_refuses_all_remaining_queries(self):
        scratch, out = self.root / "scratch", self.root / "evidence"
        scratch.mkdir(); out.mkdir()
        emulator = Mock(adb=Path("/sdk/platform-tools/adb"))
        emulator.owned.side_effect = [None, None, RuntimeError("private identity refusal")]
        def command(argv, path, **options):
            path.write_bytes(b"owned guest diagnostic")
        with patch.object(runner.runtime, "command", side_effect=command) as query:
            result = runner.preserve_guest_anr(emulator, scratch, out)
        self.assertEqual(query.call_count, 2)
        self.assertEqual(emulator.owned.call_count, 3)
        self.assertEqual(result["queries"]["guest-anr-events"], {"errorType": "RuntimeError"})
        self.assertNotIn("guest-system-app-anr", result["queries"])
        self.assertNotIn("private identity refusal", str(result))
        self.assertFalse((scratch / "guest-anr-events-raw.log").exists())
        self.assertFalse((out / "guest-system-app-anr.log").exists())

    def test_guest_ownership_loss_before_ui_state_refuses_window_and_crash_reads(self):
        scratch, out = self.root / "scratch", self.root / "evidence"
        scratch.mkdir(); out.mkdir()
        emulator = Mock(adb=Path("/sdk/platform-tools/adb"))
        emulator.owned.side_effect = [None] * 4 + [RuntimeError("private identity refusal")]
        def command(argv, path, **options):
            path.write_bytes(b"owned guest diagnostic")
        with patch.object(runner.runtime, "command", side_effect=command) as query:
            result = runner.preserve_guest_anr(emulator, scratch, out)
        self.assertEqual(query.call_count, 4)
        self.assertEqual(emulator.owned.call_count, 5)
        self.assertEqual(result["queries"]["guest-activity-state"], {"errorType": "RuntimeError"})
        self.assertNotIn("guest-window-state", result["queries"])
        self.assertNotIn("guest-crash", result["queries"])
        self.assertNotIn("private identity refusal", str(result))
        self.assertFalse((scratch / "guest-activity-state-raw.log").exists())

    def test_guest_anr_read_refuses_foreign_ownership_and_preserves_query_failure(self):
        scratch, out = self.root / "scratch", self.root / "evidence"
        scratch.mkdir(); out.mkdir()
        emulator = Mock(adb=Path("/sdk/platform-tools/adb"))
        emulator.owned.side_effect = RuntimeError("foreign guest")
        with patch.object(runner.runtime, "command") as command:
            result = runner.preserve_guest_anr(emulator, scratch, out)
        command.assert_not_called()
        self.assertEqual(result["queries"]["guest-last-anr"]["errorType"], "RuntimeError")
        emulator.owned.side_effect = None
        with patch.object(runner.runtime, "command", side_effect=TimeoutError):
            result = runner.preserve_guest_anr(emulator, scratch, out)
        self.assertEqual(result["queries"]["guest-last-anr"]["errorType"], "TimeoutError")
        self.assertNotIn("passed", result)

    def test_interrupted_guest_anr_read_keeps_partial_bytes_without_reporting_completion(self):
        scratch, out = self.root / "scratch", self.root / "evidence"
        scratch.mkdir(); out.mkdir()
        emulator = Mock(adb=Path("/sdk/platform-tools/adb"))
        def interrupted(argv, path, **options):
            path.write_bytes(b"partial guest diagnostic")
            raise TimeoutError("private exception detail")
        with patch.object(runner.runtime, "command", side_effect=interrupted):
            result = runner.preserve_guest_anr(emulator, scratch, out)
        for name, entry in result["queries"].items():
            self.assertEqual(entry["errorType"], "TimeoutError")
            self.assertNotIn("queryReturned", entry)
            self.assertEqual((out / (name + ".log")).read_bytes(), b"partial guest diagnostic")
        self.assertNotIn("private exception detail", str(result))

    # native-observation-begin:collector-controls
    def native_prior(self):
        names = ("guest-last-anr", "guest-last-anr-traces", "guest-anr-events", "guest-system-app-anr",
                 "guest-activity-state", "guest-window-state", "guest-crash")
        value = {"queries": {name: {"queryReturned": True} for name in names}}
        value["queries"]["guest-activity-state"]["appProcessPid"] = 42
        return value

    def native_line(self, seq=1, stage="BACK_ENTER", cap=0):
        return (f"v=1 seq={seq} pid=42 t=123 stage={stage} view=77 cur=1 host=1 gen=0 "
                f"vis=1 res=1 evict=0 logical=UNKNOWN physical=unknown cap={cap}\n").encode()

    def activity_snapshot(self):
        return (b"  topResumedActivity=ActivityRecord{203787634 u0 com.takosumi.fixture.android/com.takosumi.appscreen.AppScreenActivity t9}\n"
                b"  * Hist  #0: ActivityRecord{203787634 u0 com.takosumi.fixture.android/com.takosumi.appscreen.AppScreenActivity t9}\n"
                b"    packageName=com.takosumi.fixture.android processName=com.takosumi.fixture.android\n"
                b"    app=ProcessRecord{bacc64 3169:com.takosumi.fixture.android/u0a216}\n"
                b"    mActivityComponent=com.takosumi.fixture.android/com.takosumi.appscreen.AppScreenActivity\n"
                b"  * Task summary follows\n")

    def test_native_pid_projection_requires_one_complete_matching_activity_process(self):
        data = self.activity_snapshot()
        self.assertEqual(runner.native_activity_pid(data), 3169)
        self.assertIsNone(runner.native_activity_pid(data, truncated=True))
        invalid = [b"", data[:data.index(b"  * Task")], data + data,
                   data.replace(b"203787634", b"abc", 1),
                   data.replace(b"processName=com.takosumi.fixture.android", b"processName=foreign"),
                   data.replace(b"mActivityComponent=com.takosumi", b"mActivityComponent=foreign"),
                   data.replace(b"/u0a216", b"/u1a216"),
                   data.replace(b"3169:", b"0:"), data.replace(b"3169:", b"2147483648:"),
                   data.replace(b"    app=", b"    app=ProcessRecord{aaaa 3170:com.takosumi.fixture.android/u0a216}\n    app="),
                   data.replace(b"    app=", b"  * Hist  #1: ActivityRecord{abc u0 other/Activity t1}\n    app="),
                   data.replace(b"topResumedActivity=", b"topResumedActivity=\xff")]
        for bad in invalid:
            with self.subTest(data=bad):
                self.assertIsNone(runner.native_activity_pid(bad))

    def test_native_pid_scope_reuses_existing_snapshot_and_refuses_unknown_without_lookup(self):
        scratch, out = self.root / "scratch", self.root / "out"
        scratch.mkdir(); out.mkdir()
        emulator = Mock(adb=Path("/sdk/platform-tools/adb"))
        calls = []
        def command(argv, path, **options):
            calls.append(argv)
            if "activities" in argv:
                path.write_bytes(self.activity_snapshot())
            elif "--pid=3169" in argv:
                path.write_bytes(self.native_line().replace(b"pid=42", b"pid=3169") + self.native_line())
            else:
                path.write_bytes(b"existing diagnostic\n")
        with patch.object(runner.runtime, "command", side_effect=command):
            prior = runner.preserve_guest_anr(emulator, scratch, out)
            value = runner.preserve_native_observation(emulator, scratch, out, prior)
        self.assertEqual(prior["queries"]["guest-activity-state"].get("appProcessPid"), 3169)
        self.assertEqual(len(calls), 8, "Existing seven reads plus the one narrowed native query only")
        self.assertEqual(calls[-1][-4:], ["--pid=3169", "-s", "TakosWebHomeNative:I", "*:S"])
        self.assertEqual([row["pid"] for row in value["records"]], [3169])
        self.assertEqual(value["pidMismatchPackets"], 1)
        self.assertEqual(value["coverage"], "unknown")
        self.assertEqual(value["captureWindow"], "pid_main_tail_then_closed_tag_filter")
        owned_before_unknown = emulator.owned.call_count
        for pid in (None, 0, -1, 2147483648, True, "3169"):
            with self.subTest(pid=pid):
                unknown = self.native_prior()
                unknown["queries"]["guest-activity-state"]["appProcessPid"] = pid
                with patch.object(runner.runtime, "command") as query:
                    skipped = runner.preserve_native_observation(emulator, scratch, out, unknown)
                query.assert_not_called()
                self.assertEqual(skipped["skipped"], "app_process_unknown")
                self.assertNotIn("passed", skipped)
        self.assertEqual(emulator.owned.call_count, owned_before_unknown)
        for defect in ("interrupted", "truncated"):
            with self.subTest(defect=defect):
                local_scratch, local_out = self.root / (defect + "-scratch"), self.root / (defect + "-out")
                local_scratch.mkdir(); local_out.mkdir()
                def incomplete(argv, path, **options):
                    if "activities" in argv:
                        path.write_bytes(self.activity_snapshot() + (b"x" * (128 * 1024) if defect == "truncated" else b""))
                        if defect == "interrupted": raise TimeoutError("not a complete snapshot")
                    else: path.write_bytes(b"existing diagnostic\n")
                with patch.object(runner.runtime, "command", side_effect=incomplete) as query:
                    prior = runner.preserve_guest_anr(emulator, local_scratch, local_out)
                    skipped = runner.preserve_native_observation(emulator, local_scratch, local_out, prior)
                self.assertEqual(query.call_count, 7)
                self.assertIsNone(prior["queries"]["guest-activity-state"]["appProcessPid"])
                self.assertEqual(skipped["skipped"], "app_process_unknown")

    def test_native_observation_is_one_closed_tail_query_and_strict_metadata_only(self):
        scratch, out = self.root / "scratch", self.root / "out"
        scratch.mkdir(); out.mkdir()
        emulator = Mock(adb=Path("/sdk/platform-tools/adb"))
        def command(argv, path, **options):
            path.write_bytes(self.native_line() + b"https://private.invalid/user token=private\n"
                             + self.native_line(64, "CAP_EXHAUSTED", 1))
        with patch.object(runner.runtime, "command", side_effect=command) as query:
            value = runner.preserve_native_observation(emulator, scratch, out, self.native_prior())
        emulator.owned.assert_called_once()
        query.assert_called_once_with([str(emulator.adb), "-s", runner.SERIAL, "shell", "logcat", "-b", "main",
                                       "-d", "-v", "raw", "-t", "64", "--pid=42", "-s", "TakosWebHomeNative:I", "*:S"],
                                      scratch / "guest-native-observation-raw.log", timeout=10, read_output=False)
        self.assertTrue(value["diagnosticOnly"])
        self.assertEqual((value["clockDomain"], value["clockUnit"]), ("android_elapsed_realtime", "ms"))
        self.assertEqual([row["stage"] for row in value["records"]], ["BACK_ENTER", "CAP_EXHAUSTED"])
        self.assertTrue(value["nativeCapSeen"])
        self.assertEqual(value["rejectedPackets"], 1)
        saved = out / "guest-native-observation.json"
        self.assertEqual(saved.stat().st_mode & 0o777, 0o600)
        self.assertLessEqual(saved.stat().st_size, 32 * 1024)
        self.assertNotIn(b"private", saved.read_bytes())
        self.assertEqual(value["coverage"], "unknown")
        self.assertNotIn("passed", value)
        self.assertFalse(any(out.glob("*-raw.log")))

    def test_native_observation_rejects_unknown_stages_extra_fields_and_numeric_overflow(self):
        line = self.native_line()
        bad = [line.replace(b"BACK_ENTER", b"OTHER"), line.rstrip() + b" url=https://private.invalid\n",
               line.replace(b"seq=1", b"seq=65"), line.replace(b"pid=42", b"pid=2147483648"),
               line.replace(b"t=123", b"t=9223372036854775808"),
               line.replace(b"view=77", b"view=2147483648"),
               line.replace(b"gen=0", b"gen=9223372036854775808"),
               line.replace(b"physical=unknown", b"physical=attached"),
               self.native_line(64), self.native_line(1, "CAP_EXHAUSTED", 1)]
        for raw in bad:
            with self.subTest(raw=raw):
                records, rejected, cut = runner.parse_native_observation(raw)
                self.assertEqual(records, [])
                self.assertEqual(rejected, 1)
                self.assertFalse(cut)

    def test_native_observation_has_separate_prefix_record_and_saved_byte_caps(self):
        scratch, out = self.root / "scratch", self.root / "out"
        scratch.mkdir(); out.mkdir()
        emulator = Mock(adb=Path("/sdk/platform-tools/adb"))
        body = self.native_line() * 65 + b"x" * (128 * 1024)
        def command(argv, path, **options): path.write_bytes(body)
        with patch.object(runner.runtime, "command", side_effect=command):
            value = runner.preserve_native_observation(emulator, scratch, out, self.native_prior())
        self.assertEqual(len(value["records"]), 64)
        self.assertTrue(value["rawPrefixTruncated"])
        self.assertTrue(value["recordLimitReached"])
        self.assertEqual(value["coverage"], "unknown")
        self.assertLessEqual(value["rawPrefixBytes"], 64 * 1024)
        self.assertLessEqual((out / "guest-native-observation.json").stat().st_size, 32 * 1024)

    def test_native_observation_never_retries_incomplete_or_foreign_guest_custody(self):
        scratch, out = self.root / "scratch", self.root / "out"
        scratch.mkdir(); out.mkdir()
        emulator = Mock(adb=Path("/sdk/platform-tools/adb"))
        prior = self.native_prior(); prior["queries"]["guest-crash"] = {"errorType": "RuntimeError"}
        with patch.object(runner.runtime, "command") as query:
            value = runner.preserve_native_observation(emulator, scratch, out, prior)
        emulator.owned.assert_not_called(); query.assert_not_called()
        self.assertEqual(value["skipped"], "prior_diagnostic_incomplete")
        emulator.owned.side_effect = RuntimeError("private identity detail")
        with patch.object(runner.runtime, "command") as query:
            value = runner.preserve_native_observation(emulator, scratch, out, self.native_prior())
        query.assert_not_called()
        self.assertEqual(value["error"], "ownership_or_input_refused")
        self.assertNotIn("private", str(value))

    def test_native_observation_interruption_preserves_partial_metadata_as_unknown(self):
        scratch, out = self.root / "scratch", self.root / "out"
        scratch.mkdir(); out.mkdir()
        emulator = Mock(adb=Path("/sdk/platform-tools/adb"))
        def command(argv, path, **options):
            path.write_bytes(self.native_line())
            raise KeyboardInterrupt("private interruption detail")
        with patch.object(runner.runtime, "command", side_effect=command) as query:
            value = runner.preserve_native_observation(emulator, scratch, out, self.native_prior())
        self.assertEqual(query.call_count, 1)
        self.assertFalse(value["queryReturned"])
        self.assertEqual(value["error"], "interrupted")
        self.assertEqual(len(value["records"]), 1)
        self.assertEqual(value["coverage"], "unknown")
        self.assertNotIn("private", str(value))
        self.assertNotIn("passed", value)

    def test_native_observation_refuses_symlink_existing_output_and_nonregular_raw(self):
        scratch, out = self.root / "scratch", self.root / "out"
        scratch.mkdir(); out.mkdir()
        emulator = Mock(adb=Path("/sdk/platform-tools/adb"))
        secret = self.root / "private-sentinel"; secret.write_bytes(b"never copy this")
        raw = scratch / "guest-native-observation-raw.log"; raw.symlink_to(secret)
        with patch.object(runner.runtime, "command") as query:
            value = runner.preserve_native_observation(emulator, scratch, out, self.native_prior())
        emulator.owned.assert_not_called(); query.assert_not_called(); raw.unlink()
        saved = out / "guest-native-observation.json"; saved.write_bytes(b"keep original")
        def command(argv, path, **options): path.write_bytes(self.native_line())
        with patch.object(runner.runtime, "command", side_effect=command):
            value = runner.preserve_native_observation(emulator, scratch, out, self.native_prior())
        self.assertEqual(value["collectionError"], "output_refused")
        self.assertEqual(saved.read_bytes(), b"keep original")
        raw.unlink(); saved.unlink()
        def fifo(argv, path, **options): os.mkfifo(path)
        with patch.object(runner.runtime, "command", side_effect=fifo):
            value = runner.preserve_native_observation(emulator, scratch, out, self.native_prior())
        self.assertEqual(value["collectionError"], "output_refused")
        self.assertFalse(saved.exists())
    # native-observation-end:collector-controls

    def emulator_process_fixture(self):
        emulator = runner.Emulator.__new__(runner.Emulator)
        emulator.sdk = self.root / "sdk"
        emulator.name = "sumi-whole-app-synthetic"
        emulator.birth = 100
        emulator.process = Mock(pid=100, poll=Mock(return_value=None))
        image = emulator.sdk / "emulator/qemu/linux-x86_64/qemu-system-x86_64"
        image.parent.mkdir(parents=True)
        image.write_bytes(b"synthetic SDK image")
        guard = self.root / "sumi-android-emulator-guard"
        guard.write_bytes(b"synthetic guard")
        proc = self.root / "proc"
        argv = [b"-avd", emulator.name.encode(), b"-port", b"5582"]
        for pid, executable, arguments in (
                (100, guard, [str(guard).encode(), b"guard.json", str(image).encode(), *argv]),
                (200, image, [str(image).encode(), *argv])):
            entry = proc / str(pid)
            entry.mkdir(parents=True)
            (entry / "exe").symlink_to(executable)
            (entry / "cmdline").write_bytes(b"\0".join(arguments) + b"\0")
        identities = {100: (os.getpid(), 100), 200: (100, 200)}
        return emulator, proc, identities

    def test_emulator_guard_forwarded_argv_is_not_an_sdk_child(self):
        emulator, proc, identities = self.emulator_process_fixture()
        with patch.object(runner, "Path", return_value=proc), \
                patch.object(runner, "identity", side_effect=identities.__getitem__):
            emulator.owned_process()

    def test_guard_argv_alone_cannot_establish_emulator_ownership(self):
        emulator, proc, identities = self.emulator_process_fixture()
        (proc / "200").rename(proc / "not-a-process")
        with patch.object(runner, "Path", return_value=proc), \
                patch.object(runner, "identity", side_effect=identities.__getitem__), \
                self.assertRaisesRegex(RuntimeError, "No SDK emulator"):
            emulator.owned_process()

    def test_matching_emulator_still_requires_sdk_image_and_guard_ancestry(self):
        emulator, proc, identities = self.emulator_process_fixture()
        image = proc / "200/exe"
        original = image.resolve(strict=True)
        foreign = self.root / "foreign-image"
        foreign.write_bytes(b"foreign")
        with patch.object(runner, "Path", return_value=proc), \
                patch.object(runner, "identity", side_effect=identities.__getitem__):
            image.unlink()
            image.symlink_to(foreign)
            with self.assertRaisesRegex(RuntimeError, "outside the SDK emulator"):
                emulator.owned_process()
            image.unlink()
            image.symlink_to(original)
            identities[200] = (1, 200)
            with self.assertRaisesRegex(RuntimeError, "Foreign emulator ancestry"):
                emulator.owned_process()

    def test_recycled_emulator_birth_cannot_pass_ownership_readback(self):
        emulator, proc, identities = self.emulator_process_fixture()
        reads = 0
        def identity(pid):
            nonlocal reads
            if pid == 200:
                reads += 1
                if reads > 1:
                    return 100, 201
            return identities[pid]
        with patch.object(runner, "Path", return_value=proc), \
                patch.object(runner, "identity", side_effect=identity), \
                self.assertRaisesRegex(RuntimeError, "Unstable emulator ancestry"):
            emulator.owned_process()

    def network(self, config=CONFIG, table=TABLE, cert=CERT, tree=XMLTREE):
        app = ET.Element("application", {ANDROID + "networkSecurityConfig": "@ref/0x7f120003"})
        archive_bytes = io.BytesIO()
        with zipfile.ZipFile(archive_bytes, "w") as archive:
            archive.writestr("res/raw/sumi_fixture_ca.pem", cert)
        with zipfile.ZipFile(archive_bytes) as archive:
            return verify_network(app, table, lambda _: ET.fromstring(config), archive, self.cert, lambda _: tree)

    def test_actual_xmltree_text_restores_only_omitted_decoded_domain_leaves(self):
        root = ET.fromstring(CONFIG)
        for domain in root.findall("domain-config/domain"):
            domain.text = None
        result = domain_text_from_xmltree(root, XMLTREE)
        self.assertEqual([n.text for n in result.findall("domain-config/domain")], ["127.0.0.1", "localhost"])
        self.assertEqual([n.text for n in root.findall("domain-config/domain")], [None, None])
        missing = CONFIG.replace(">127.0.0.1<", "><").replace(">localhost<", "><")
        self.network(missing)

    def test_domain_text_cannot_be_fabricated_from_absence_or_another_node(self):
        root = ET.fromstring(CONFIG)
        for domain in root.findall("domain-config/domain"): domain.text = None
        for tree in (XMLTREE.replace("            T: '127.0.0.1'\n", ""),
                     XMLTREE.replace("T: 'localhost'", "T: ''"),
                     XMLTREE.replace("T: 'localhost'", "T: 'localhost'\n            T: 'localhost'"),
                     XMLTREE.replace("            T: 'localhost'", "        T: 'localhost'"),
                     XMLTREE + "    T: '127.0.0.1'\n",
                     XMLTREE.replace("    E: domain-config", "  E: domain-config"),
                     XMLTREE.replace("E: domain-config", "E: other"),
                     "N: prefix=http://foreign\n" + XMLTREE):
            with self.subTest(tree=tree), self.assertRaises(RuntimeError): domain_text_from_xmltree(root, tree)

    def test_nonempty_decoded_text_must_agree_with_the_independent_packaged_text(self):
        with self.assertRaises(RuntimeError):
            domain_text_from_xmltree(ET.fromstring(CONFIG), XMLTREE.replace("T: 'localhost'", "T: 'evil.example'"))

    def test_independent_text_does_not_replace_the_closed_host_policy(self):
        root = ET.fromstring(CONFIG)
        for domain in root.findall("domain-config/domain"): domain.text = None
        changed = domain_text_from_xmltree(root, XMLTREE.replace("T: 'localhost'", "T: 'evil.example'"))
        self.assertEqual(changed.findall("domain-config/domain")[1].text, "evil.example")
        self.assertNotEqual(changed.findall("domain-config/domain")[1].text, "localhost")
        empty = CONFIG.replace(">127.0.0.1<", "><").replace(">localhost<", "><")
        with self.assertRaisesRegex(RuntimeError, "Fixture trust must be limited"):
            self.network(empty, tree=XMLTREE.replace("T: 'localhost'", "T: 'evil.example'"))

    def test_independent_tree_cannot_hide_missing_extra_or_duplicate_attributes(self):
        for tree in (XMLTREE.replace('              A: src=@0x7f130001\n', ''),
                     XMLTREE + '              A: src=@0x7f130001\n',
                     XMLTREE + '              A: extra="anything"\n'):
            with self.subTest(tree=tree), self.assertRaises(RuntimeError):
                domain_text_from_xmltree(ET.fromstring(CONFIG), tree)

    def test_compiled_policy_and_packaged_ca_must_match_this_run(self):
        proof = self.network(CONFIG.replace("@raw/sumi_fixture_ca", "@ref/0x7f130001"))
        self.assertEqual(proof["domains"], ["127.0.0.1", "localhost"])
        with self.assertRaises(RuntimeError):
            self.network(cert=CERT.replace(b"c3lu", b"AAAA"))
        with self.assertRaises(ValueError):
            self.network(table=TABLE.replace("res/raw/sumi_fixture_ca.pem", "res/raw/foreign.pem"))

    def test_wider_host_or_system_trust_or_cleartext_is_refused(self):
        for config in (CONFIG.replace('includeSubdomains="false"', 'includeSubdomains="true"'),
                       CONFIG.replace("localhost", "example.com"),
                       CONFIG.replace('cleartextTrafficPermitted="false"', 'cleartextTrafficPermitted="true"'),
                       CONFIG.replace("</trust-anchors>", '<certificates src="system" /></trust-anchors>'),
                       CONFIG.replace("</network-security-config>", "<base-config /></network-security-config>")):
            with self.subTest(config=config), self.assertRaises((RuntimeError, ValueError)):
                self.network(config)

    def test_mismatched_decoded_policy_is_preserved_without_certificate_contents(self):
        with self.assertRaises(RuntimeError) as caught:
            self.network(CONFIG.replace('includeSubdomains="false"', 'includeSubdomains="true"'))
        message = str(caught.exception)
        self.assertIn('"actual"', message)
        self.assertIn('"expected"', message)
        self.assertIn('"includeSubdomains","true"', message)
        self.assertNotIn(CERT.decode(), message)

    def test_only_a_plain_single_public_cert_can_be_packaged(self):
        self.assertEqual(certificate_bytes(self.cert), CERT)
        for value in (CERT + CERT, CERT.replace(b"CERTIFICATE", b"PRIVATE KEY"), b"", CERT + b"garbage"):
            self.cert.write_bytes(value)
            with self.subTest(value=value), self.assertRaises(RuntimeError):
                certificate_bytes(self.cert)
        self.cert.write_bytes(CERT)
        link = self.root / "linked.pem"
        link.symlink_to(self.cert)
        with self.assertRaises(RuntimeError): certificate_bytes(link)
        link.unlink()
        os.link(self.cert, link)
        with self.assertRaises(RuntimeError): certificate_bytes(self.cert)

    def test_overlay_exactly_restores_the_owned_manifest_and_only_its_resources(self):
        _, main, manifest = self.project()
        before = manifest.read_bytes()
        unrelated = main / "res/xml/unrelated.xml"
        unrelated.write_text("keep this owner input")
        overlay = Overlay(self.root, self.cert)
        overlay.apply()
        self.assertEqual(ET.parse(manifest).getroot().find("application").get(ANDROID + "networkSecurityConfig"),
                         "@xml/sumi_fixture_network_security")
        overlay.restore()
        self.assertEqual(manifest.read_bytes(), before)
        self.assertEqual(unrelated.read_text(), "keep this owner input")
        self.assertFalse((main / "res/raw/sumi_fixture_ca.pem").exists())

    def test_changed_manifest_or_resource_is_preserved_and_restoration_refused(self):
        _, _, manifest = self.project()
        overlay = Overlay(self.root, self.cert)
        overlay.apply()
        for path in (manifest, *overlay.files):
            before = path.read_bytes()
            path.write_bytes(before + b"changed")
            with self.subTest(path=path), self.assertRaises(RuntimeError): overlay.restore()
            self.assertEqual(path.read_bytes(), before + b"changed")
            path.write_bytes(before)
        overlay.restore()

    def test_replaced_resource_parent_is_refused_even_with_identical_bytes(self):
        _, main, _ = self.project()
        overlay = Overlay(self.root, self.cert)
        overlay.apply()
        raw = main / "res/raw"
        raw.rename(main / "res/original-raw")
        raw.mkdir()
        (raw / "sumi_fixture_ca.pem").write_bytes(CERT)
        with self.assertRaises(RuntimeError): overlay.restore()
        self.assertEqual((raw / "sumi_fixture_ca.pem").read_bytes(), CERT)

    def test_existing_policy_or_resource_or_foreign_package_is_never_replaced(self):
        generated, main, manifest = self.project()
        original = manifest.read_text()
        manifest.write_text(original.replace("<application ", '<application android:networkSecurityConfig="@xml/owned" '))
        with self.assertRaises(RuntimeError): Overlay(self.root, self.cert)
        manifest.write_text(original)
        resource = main / "res/xml/sumi_fixture_network_security.xml"
        resource.write_text("owner input")
        with self.assertRaises(RuntimeError): Overlay(self.root, self.cert)
        self.assertEqual(resource.read_text(), "owner input")
        resource.unlink()
        (generated / "app/build.gradle.kts").write_text('applicationId = "com.takosumi"')
        with self.assertRaises(RuntimeError): Overlay(self.root, self.cert)

    def test_installed_apk_identity_is_not_a_package_name_only_check(self):
        emulator = Mock()
        path = "/data/app/~~abc/com.takosumi.fixture.android-abc/base.apk"
        sha = "a" * 64
        emulator.run.side_effect = ["package:" + path + "\n", sha + "  " + path + "\n"]
        self.assertEqual(runner.installed(emulator, sha), sha)
        emulator.run.side_effect = ["package:" + path + "\n", "b" * 64 + "  " + path + "\n"]
        with self.assertRaises(RuntimeError): runner.installed(emulator, sha)
        for output in ("", "package:/outside/base.apk", "package:" + path + "\npackage:" + path):
            emulator.run.side_effect = [output]
            with self.subTest(output=output), self.assertRaises(RuntimeError): runner.installed(emulator, sha)

    def test_separate_driver_identity_cannot_substitute_a_product_launcher(self):
        paths = [self.root / "driver.apk", self.root / "test.apk"]
        for path in paths: path.write_bytes(b"synthetic SDK-free APK stand-in")
        def manifest(package, extra=""):
            return '<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="' + package + '">' + extra + '</manifest>'
        normal = manifest(runner.DRIVER, '<application />')
        test = manifest(runner.DRIVER + '.test', '<instrumentation android:targetPackage="' + runner.DRIVER
                        + '" android:name="androidx.test.runner.AndroidJUnitRunner" />')
        command = Mock(side_effect=[normal, "", test, ""])
        self.assertEqual(len(runner.driver_apks(paths, self.root, command)), 2)
        for a, b in ((normal.replace(runner.DRIVER, runner.PACKAGE), test),
                     (normal, test.replace('targetPackage="' + runner.DRIVER, 'targetPackage="' + runner.PACKAGE)),
                     (normal.replace('<application />', '<application><activity><intent-filter>'
                        '<action android:name="android.intent.action.MAIN" /></intent-filter></activity></application>'), test)):
            command = Mock(side_effect=[a, "", b, ""])
            with self.subTest(a=a, b=b), self.assertRaises(RuntimeError): runner.driver_apks(paths, self.root, command)

    def test_wrong_avd_identity_refuses_before_any_mutating_adb_call(self):
        emulator = runner.Emulator(self.root, self.root, self.root, Mock(return_value="foreign\nOK\n"))
        emulator.owned_process = Mock()
        with self.assertRaises(RuntimeError): emulator.run("shell", "am", "force-stop", runner.PACKAGE)
        self.assertEqual(emulator.command.call_count, 1)
        self.assertEqual(emulator.command.call_args.args[0][-3:], ["emu", "avd", "name"])

    def test_apk_restore_uses_bound_plain_files_and_refuses_same_hash_replacement(self):
        original, variant = b"normal SDK APK bytes", b"fixture SDK APK bytes"
        baseline_hash, variant_hash = (hashlib.sha256(value).hexdigest() for value in (original, variant))
        apk, backup = self.root / "variant.apk", self.root / "normal.apk"
        apk.write_bytes(variant)
        backup.write_bytes(original)
        destination, source = runner.FileCustody(apk, self.root), runner.FileCustody(backup, self.root)
        apk.rename(self.root / "old-variant.apk")
        apk.write_bytes(variant)
        with self.assertRaises(RuntimeError): runner.restore_apk(destination, source, variant_hash, baseline_hash)
        self.assertEqual(apk.read_bytes(), variant)
        apk.unlink()
        foreign = self.root / "foreign.apk"
        foreign.write_bytes(variant)
        apk.symlink_to(foreign)
        with self.assertRaises(RuntimeError): runner.restore_apk(destination, source, variant_hash, baseline_hash)
        self.assertEqual(foreign.read_bytes(), variant)
        apk.unlink()
        (self.root / "old-variant.apk").rename(apk)
        runner.restore_apk(destination, source, variant_hash, baseline_hash)
        self.assertEqual(apk.read_bytes(), original)

    def test_apk_backup_and_parent_custody_are_required_before_any_write(self):
        folder = self.root / "build"
        folder.mkdir()
        apk, backup = folder / "app.apk", self.root / "normal.apk"
        apk.write_bytes(b"variant")
        backup.write_bytes(b"normal")
        target, source = runner.FileCustody(apk, self.root), runner.FileCustody(backup, self.root)
        backup.rename(self.root / "previous.apk")
        backup.write_bytes(b"normal")
        with self.assertRaises(RuntimeError): runner.restore_apk(target, source, "", "")
        self.assertEqual(apk.read_bytes(), b"variant")
        source = runner.FileCustody(backup, self.root)
        folder.rename(self.root / "old-build")
        folder.mkdir()
        apk.write_bytes(b"variant")
        with self.assertRaises(RuntimeError): runner.restore_apk(target, source, "", "")
        self.assertEqual(apk.read_bytes(), b"variant")


if __name__ == "__main__":
    unittest.main()
