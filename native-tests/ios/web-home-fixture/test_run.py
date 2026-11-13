"""Harness/protocol controls only; these do not execute or qualify the iOS App."""
import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import plistlib
import re
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import types
import unittest
from unittest.mock import patch
from urllib.request import build_opener, HTTPSHandler, HTTPCookieProcessor, ProxyHandler

import run as runner
from harness import runtime
from web_site import Handler, Site


class Controls(unittest.TestCase):
    def setUp(self):
        parent = Path(os.environ.get("TMPDIR", "/root/dev/tmp")).resolve()
        self.assertFalse(parent.is_relative_to(runner.ROOT), "Private mock material belongs outside the repository")
        self.root = Path(self.enterContext(tempfile.TemporaryDirectory(dir=parent)))

    def test_whole_app_build_and_ui_use_one_ci_host_independent_of_download(self):
        workflow = (runner.ROOT / ".github/workflows/ci.yml").read_text().split("\njobs:\n", 1)[1]
        starts = list(re.finditer(r"(?m)^  ([a-z][a-z0-9-]*):\n", workflow))
        jobs = {start[1]: workflow[start.end():starts[i + 1].start() if i + 1 < len(starts) else len(workflow)]
                for i, start in enumerate(starts)}
        sdk = "run: bash native-tests/ios/build-simulator.sh"
        ui = "run: python3 -B native-tests/ios/web-home-fixture/run.py"
        download = "run: python3 -B native-tests/ios/download-fixture/run.py"
        self.assertEqual([workflow.count(command) for command in (sdk, ui, download)], [1, 1, 1],
                         "Build and each native case must execute exactly once")
        owners = [[name for name, body in jobs.items() if command in body] for command in (sdk, ui, download)]
        self.assertEqual([len(names) for names in owners], [1, 1, 1], "Each native command needs one owning job")
        self.assertEqual(owners[0], owners[1], "The UI must bind the SDK built in its own job")
        self.assertNotEqual(owners[1], owners[2], "Whole App and download must not share a host/CoreSimulator service")
        body = jobs[owners[1][0]]
        self.assertRegex(body, r"(?m)^    runs-on: macos-26$", "Whole-App SDK/UI use the native arm64 host")
        self.assertNotRegex(body, r"(?m)^    strategy:", "A matrix must not repeat the whole-App case")
        self.assertLess(body.index(sdk), body.index(ui))
        self.assertIn("steps.ios-sdk.outcome == 'success'", body)
        self.assertIn("name: ios-app-swift-runtime", body)
        self.assertIn("name: ios-whole-app-web-home", body)
        self.assertNotIn("actions/download-artifact@", body, "Keep SDK input and installed-App readback on one host")
        standalone = jobs[owners[2][0]]
        for setup in ("brew install xcodegen", "xcodegen --version"):
            self.assertTrue(setup in standalone, "The standalone host must prepare XcodeGen without the SDK init: " + setup)
            self.assertLess(standalone.index(setup), standalone.index(download))

    def app(self, root=None):
        root = root or self.root / "build"
        app = root / "Takosumi.app"
        app.mkdir(parents=True)
        info = {"CFBundleIdentifier": runner.BUNDLE, "CFBundleSupportedPlatforms": ["iPhoneSimulator"],
                "CFBundleExecutable": "Takosumi"}
        with (app / "Info.plist").open("wb") as out:
            plistlib.dump(info, out)
        (app / "Takosumi").write_bytes(b"synthetic executable bytes; never installed")
        assets = app / "assets"
        assets.mkdir()
        (assets / "index.html").write_text("synthetic bundled UI")
        return app, root, info

    def test_bundle_attestation_includes_frontend_resources_and_refuses_wrong_identity(self):
        app, root, info = self.app()
        proof = runner.bundle_proof(app, root)
        (app / "assets/index.html").write_text("changed UI with the same executable")
        changed = runner.bundle_proof(app, root)
        self.assertEqual(proof["executableSHA256"], changed["executableSHA256"])
        self.assertNotEqual(proof["treeSHA256"], changed["treeSHA256"])
        for key, value in (("CFBundleIdentifier", "com.takosumi"), ("CFBundleSupportedPlatforms", ["iPhoneOS"]),
                           ("CFBundleExecutable", "../outside")):
            wrong = dict(info, **{key: value})
            with (app / "Info.plist").open("wb") as out:
                plistlib.dump(wrong, out)
            with self.assertRaises(RuntimeError):
                runner.bundle_proof(app, root)

    def test_foreign_bundle_and_linked_resources_are_refused(self):
        app, root, _ = self.app()
        other = self.root / "other"
        other.mkdir()
        with self.assertRaises(RuntimeError):
            runner.bundle_proof(app, other)
        (app / "linked").symlink_to(app / "Takosumi")
        with self.assertRaises(RuntimeError):
            runner.bundle_proof(app, root)

    def test_installed_app_must_be_inside_the_exact_created_device_and_match_all_bytes(self):
        udid = "11111111-1111-4111-8111-111111111111"
        app, build, _ = self.app()
        proof = runner.bundle_proof(app, build)
        data = self.root / "Devices" / udid / "data"
        installed, _, _ = self.app(data / "Containers/Bundle/Application/owned")
        device = {"dataPath": str(data)}
        with patch.object(runtime, "command", return_value=str(installed)):
            self.assertEqual(runner.installed_app(self.root, udid, device, proof, "before"), proof)
            (installed / "assets/index.html").write_text("replaced frontend")
            with self.assertRaises(RuntimeError):
                runner.installed_app(self.root, udid, device, proof, "after")
        with patch.object(runtime, "command", return_value=str(app)):
            with self.assertRaises(RuntimeError):
                runner.installed_app(self.root, udid, device, proof, "foreign")
        with patch.object(runtime, "command", return_value=str(installed)):
            with self.assertRaises(RuntimeError):
                runner.installed_app(self.root, "22222222-2222-4222-8222-222222222222", device, proof, "wrong-device")

    def test_exact_nonempty_xctest_is_required_without_skip_restart_or_another_case(self):
        identity = f"-[WebHomeUITests.WebHomeUITests {runner.METHOD}]"
        valid = (f"Test Case '{identity}' started.\nTest Case '{identity}' passed (1.250 seconds).\n"
                 "Executed 1 test, with 0 failures (0 unexpected)\n")
        self.assertEqual(runner.require_ui_pass(valid)["seconds"], 1.25)
        for log in ("", valid.replace("started", "skipped"), valid + valid,
                    valid.replace("passed", "failed"), valid.replace("Executed 1", "Executed 0"),
                    valid + "Test Case '-[Other other]' started.\n"):
            with self.assertRaises(RuntimeError):
                runner.require_ui_pass(log)

    def test_server_oracle_requires_authenticated_profiles_and_no_cookie_at_other_host(self):
        instance = [{"path": "/", "user": None} for _ in range(3)]
        for user in ("Work", "Personal"):
            instance += [{"path": "/sign-in/" + user, "user": None}, {"path": "/", "user": user}]
        instance.append({"path": "/account", "user": "Work"})
        instance.append({"path": "/sign-in/Work", "user": None})
        editor = [{"path": "/editor", "user": None, "cookiePresent": False} for _ in range(4)]
        editor.append({"path": "/editor/page-two", "user": None, "cookiePresent": False})
        runner.require_site_events(instance, editor)
        for defect in ("missing-user", "missing-management", "no-app", "cookie-leaked"):
            a, b = copy.deepcopy(instance), copy.deepcopy(editor)
            if defect == "missing-user":
                a = [event for event in a if event.get("user") != "Personal"]
            elif defect == "missing-management":
                a = [event for event in a if event["path"] != "/account"]
            elif defect == "no-app":
                b = []
            else:
                b[0]["cookiePresent"] = True
            with self.assertRaises(RuntimeError):
                runner.require_site_events(a, b)

    def test_cleanup_refuses_ownership_mismatch_before_shutdown_or_delete(self):
        device = {"udid": "owned", "name": "different-name", "state": "Booted"}
        with patch.object(runtime, "command", return_value=json.dumps({"devices": {"runtime": [device]}})) as command:
            with self.assertRaises(RuntimeError):
                runner.retire_device(self.root, "owned", "expected-name", "runtime")
            self.assertEqual(command.call_count, 1)
            self.assertEqual(command.call_args.args[0], ["xcrun", "simctl", "list", "devices", "--json"])

    def test_cleanup_uses_only_the_exact_created_id_and_requires_deletion_readback(self):
        before = {"devices": {"runtime": [{"udid": "owned", "name": "expected", "state": "Booted"}]}}
        with patch.object(runtime, "command", side_effect=[json.dumps(before), "", "", '{"devices":{}}']) as command:
            runner.retire_device(self.root, "owned", "expected", "runtime")
            self.assertEqual(command.call_args_list[1].args[0], ["xcrun", "simctl", "shutdown", "owned"])
            self.assertEqual(command.call_args_list[2].args[0], ["xcrun", "simctl", "delete", "owned"])
        with patch.object(runtime, "command", side_effect=[json.dumps(before), "", "", json.dumps(before)]):
            with self.assertRaises(RuntimeError):
                runner.retire_device(self.root, "owned", "expected", "runtime")

    def test_target_log_read_is_scoped_to_the_owned_booted_simulator_and_private_app(self):
        out, scratch = self.root / "evidence", self.root / "scratch"
        out.mkdir(); scratch.mkdir()
        device = {"udid": "owned", "name": "expected", "state": "Booted", "dataPath": "/owned/data", "logPath": "/owned/logs"}
        calls = []
        def command(argv, path, **kwargs):
            calls.append((argv, kwargs))
            if argv == ["xcrun", "simctl", "list", "devices", "--json"]:
                return json.dumps({"devices": {"runtime": [device]}})
            path.write_bytes(b"native target event\n")
            return "native target event\n"
        with patch.object(runtime, "command", side_effect=command):
            result = runner.preserve_app_exit(out, scratch, "owned", device, "expected", "runtime")
        self.assertNotIn("errorType", result)
        self.assertTrue(result["queryReturned"])
        self.assertFalse(result["partial"])
        self.assertEqual((out / "target-app-exit.log").read_bytes(), b"native target event\n")
        argv, options = calls[1]
        self.assertEqual(argv[:5], ["xcrun", "simctl", "spawn", "owned", "log"])
        self.assertEqual(argv[-1], '((process == "Takosumi") OR (eventMessage CONTAINS "com.takosumi.fixture.simulator")) AND (subsystem != "com.apple.dt.xctest")')
        self.assertEqual(result["excludedSubsystems"], ["com.apple.dt.xctest"])
        self.assertEqual(options["timeout"], 15)
        self.assertIs(options["read_output"], False)

    def test_target_log_read_refuses_foreign_changed_or_stopped_device_before_spawn(self):
        original = {"udid": "owned", "name": "expected", "state": "Booted", "dataPath": "/owned/data", "logPath": "/owned/logs"}
        for key, value in (("udid", "foreign"), ("name", "foreign"), ("state", "Shutdown"),
                           ("dataPath", "/foreign/data"), ("logPath", "/foreign/logs")):
            device = dict(original, **{key: value})
            with self.subTest(key=key), patch.object(runtime, "command", return_value=json.dumps({"devices": {"runtime": [device]}})) as command:
                result = runner.preserve_app_exit(self.root, self.root, "owned", original, "expected", "runtime")
                self.assertIn("errorType", result)
                self.assertEqual(command.call_count, 1)

    def test_target_log_evidence_is_bounded_and_diagnostic_failure_is_not_a_pass(self):
        out, scratch = self.root / "evidence", self.root / "scratch"
        out.mkdir(); scratch.mkdir()
        device = {"udid": "owned", "name": "expected", "state": "Booted", "dataPath": "/owned/data", "logPath": "/owned/logs"}
        def command(argv, path, **kwargs):
            if "spawn" not in argv:
                return json.dumps({"devices": {"runtime": [device]}})
            path.write_bytes(b"x" * (1024 * 1024) + b"last event")
            return ""
        with patch.object(runtime, "command", side_effect=command):
            result = runner.preserve_app_exit(out, scratch, "owned", device, "expected", "runtime")
        self.assertTrue(result["diagnosticOnly"] and result["truncated"])
        self.assertEqual(result["keptBytes"], 512 * 1024)
        self.assertTrue((out / "target-app-exit.log").read_bytes().endswith(b"last event"))
        with patch.object(runtime, "command", side_effect=TimeoutError):
            result = runner.preserve_app_exit(out, scratch, "owned", device, "expected", "runtime")
        self.assertEqual(result["errorType"], "TimeoutError")
        self.assertNotIn("passed", result)

    def test_interrupted_owned_target_query_keeps_partial_tail_and_original_failure(self):
        out, scratch = self.root / "evidence", self.root / "scratch"
        out.mkdir(); scratch.mkdir()
        device = {"udid": "owned", "name": "expected", "state": "Booted", "dataPath": "/owned/data", "logPath": "/owned/logs"}
        def command(argv, path, **kwargs):
            if "spawn" not in argv:
                return json.dumps({"devices": {"runtime": [device]}})
            path.write_bytes(b"x" * (1024 * 1024) + b"last partial event")
            raise subprocess.TimeoutExpired(argv, 15)
        with patch.object(runtime, "command", side_effect=command) as query:
            result = runner.preserve_app_exit(out, scratch, "owned", device, "expected", "runtime")
        self.assertEqual(query.call_count, 2, "No second query after interruption")
        self.assertEqual(result["errorType"], "TimeoutExpired")
        self.assertTrue((out / "target-app-exit.log").is_file(), "Keep output already written before interruption")
        self.assertFalse(result["queryReturned"])
        self.assertTrue(result["partial"])
        self.assertTrue(result["truncated"])
        self.assertEqual(result["keptBytes"], 512 * 1024)
        self.assertTrue((out / "target-app-exit.log").read_bytes().endswith(b"last partial event"))
        self.assertNotIn("passed", result)

    def test_navigation_witness_has_only_closed_metadata_and_separates_process_view_identity(self):
        payload = (b"com.takosumi.fixture.simulator.nav v=1 seq=1 event=request view=1 request=1 relation=0 "
                   b"target=editor reused=0 kept=0 resume=1 initial=0 sameView=1 timeMs=123")
        header = b"2026-11-13 00:00:00.001 Df Takosumi[%d:ab] [com.takosumi.fixture.simulator.navigation:navigation] "
        data = header % 10 + payload + b"\n" + header % 11 + payload + b"\n"
        result = runner.navigation_witness(data)
        self.assertEqual([v["pid"] for v in result["records"]], [10, 11])
        self.assertEqual([v["target"] for v in result["records"]], ["editor", "editor"])
        self.assertEqual(result["clockDomain"], "ios_system_uptime")
        self.assertTrue(result["absenceIsUnknown"])
        for bad in (b"target=https://private.example?token=secret", b"target=Work", b"view=65536", b"relation=3"):
            key = bad.split(b"=", 1)[0] + b"="
            poisoned = re.sub(key + rb"[^ ]+", bad, payload)
            parsed = runner.navigation_witness(header % 10 + poisoned)
            self.assertEqual(parsed["records"], [])
            self.assertEqual(parsed["malformed"], 1)
            self.assertNotIn("secret", json.dumps(parsed))
        for prefix in (b"", header % 10 + b"page text ", (header % 10).replace(b".simulator.navigation:navigation", b".WebKit:Network")):
            unknown = runner.navigation_witness(prefix + payload)
            self.assertEqual(unknown["records"], [], "Do not accept marker-only, spoofed text or a foreign subsystem")
            self.assertEqual(unknown["malformed"], 1)
        self.assertNotIn("passed", result)

    def test_navigation_witness_callback_context_requires_matching_view_and_navigation(self):
        header = (b"2026-11-13 00:00:00.001 Df Takosumi[42:ab] "
                  b"[com.takosumi.fixture.simulator.navigation:navigation] ")
        def packet(event="commit", relation=1, same_view=1, request=4, target="editor", flags="reused=1 kept=0 resume=1 initial=1"):
            return header + (f"com.takosumi.fixture.simulator.nav v=1 seq=4 event={event} view=2 request={request} "
                             f"relation={relation} target={target} {flags} sameView={same_view} timeMs=234\n").encode()
        for relation, same_view in ((1, 0), (0, 1), (2, 1)):
            with self.subTest(relation=relation, sameView=same_view):
                value = runner.navigation_witness(packet(relation=relation, same_view=same_view,
                                                        request=0, target="unknown"))
                self.assertEqual(value["records"], [])
                self.assertEqual(value["malformed"], 1)
        matched = runner.navigation_witness(packet())["records"][0]
        self.assertTrue(matched["reused"] and matched["resume"] and matched["initial"])
        unknown = runner.navigation_witness(packet(same_view=0, request=0, target="unknown",
                                                   flags="reused=0 kept=0 resume=0 initial=0"))["records"][0]
        self.assertEqual((unknown["request"], unknown["target"]), (0, "unknown"))
        self.assertFalse(unknown["reused"] or unknown["resume"] or unknown["initial"])
        opened = runner.navigation_witness(packet(event="open", relation=0, request=0, target="unknown"))
        self.assertEqual(len(opened["records"]), 1, "Open context does not require a returned navigation")
        returned = runner.navigation_witness(packet(event="returned", relation=0))
        self.assertEqual(len(returned["records"]), 1, "A nil return may retain the already-selected request context")
        self.assertNotIn("passed", unknown)

    def test_navigation_witness_cap_and_absence_never_grant_finish_or_pass(self):
        line = (b"2026-11-13 00:00:00.001 Df Takosumi[10:ab] [com.takosumi.fixture.simulator.navigation:navigation] "
                b"com.takosumi.fixture.simulator.nav v=1 seq=1 event=provisional view=1 request=1 relation=1 "
                b"target=editor reused=0 kept=0 resume=1 initial=0 sameView=1 timeMs=123\n")
        result = runner.navigation_witness(line * 520)
        self.assertEqual(len(result["records"]), 513)
        self.assertEqual(result["dropped"], 7)
        latest = runner.navigation_witness(line * 520 + line.replace(b"Takosumi[10:ab]", b"Takosumi[22:ef]"))
        self.assertEqual(len(latest["records"]), 513)
        self.assertEqual(latest["records"][-1]["pid"], 22, "Cold process records must survive older-process overflow")
        self.assertEqual(latest["dropped"], 8)
        self.assertEqual(result["capStatus"], "not_observed")
        marker = line.replace(b"seq=1 event=provisional", b"seq=513 event=cap")
        self.assertEqual(runner.navigation_witness(marker)["capStatus"], "observed")
        self.assertEqual(runner.navigation_witness(b"")["records"], [])
        self.assertEqual(runner.navigation_witness(b"x" * (512 * 1024 + 1))["errorType"], "InputBoundExceeded")
        self.assertNotIn("finished", result)
        self.assertNotIn("passed", result)

    def test_navigation_witness_delivery_reuses_one_partial_query_without_replacing_failure(self):
        out, scratch = self.root / "witness-evidence", self.root / "witness-scratch"
        out.mkdir(); scratch.mkdir()
        device = {"udid": "owned", "name": "expected", "state": "Booted", "dataPath": "/owned/data", "logPath": "/owned/logs"}
        line = (b"2026-11-13 00:00:00.001 Df Takosumi[11:cd] [com.takosumi.fixture.simulator.navigation:navigation] "
                b"com.takosumi.fixture.simulator.nav v=1 seq=2 event=provisional view=1 request=1 "
                b"relation=1 target=editor reused=0 kept=0 resume=1 initial=0 sameView=1 timeMs=124\n")
        def command(argv, path, **kwargs):
            if "spawn" not in argv:
                return json.dumps({"devices": {"runtime": [device]}})
            path.write_bytes(line)
            raise subprocess.TimeoutExpired(argv, 15)
        with patch.object(runtime, "command", side_effect=command) as query:
            result = runner.preserve_app_exit(out, scratch, "owned", device, "expected", "runtime")
        self.assertEqual(query.call_count, 2, "Original inventory plus original one query only")
        self.assertTrue(result["partial"])
        self.assertFalse(result["queryReturned"])
        self.assertEqual(result["navigationWitness"]["records"][0]["event"], "provisional")
        self.assertEqual(result["errorType"], "TimeoutExpired")
        self.assertNotIn("passed", result)

    def test_existing_harness_server_import_is_not_poisoned_or_left_in_global_namespace(self):
        poison = types.ModuleType("server")
        path = Path(__file__).parent / "harness.py"
        spec = importlib.util.spec_from_file_location("sumi_web_home_namespace_test", path)
        with patch.dict(sys.modules, {"server": poison}):
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            self.assertIs(sys.modules["server"], poison)
            self.assertEqual(module.runtime.FixtureServer.__module__, "sumi_ios_download_site")
            self.assertIsNot(module.runtime.INTERRUPTED_COMMANDS, runtime.INTERRUPTED_COMMANDS)

    def test_real_https_fixture_authenticates_only_the_cookie_host_and_records_closed_data(self):
        ca, cert, key = runtime.certificate(self.root, localhost=True)
        instance, editor = Site(cert, key, "instance"), Site(cert, key, "editor")
        self.addCleanup(instance.stop)
        self.addCleanup(editor.stop)
        instance.editor = editor
        instance.start()
        editor.start()
        context = ssl.create_default_context(cafile=str(ca))
        cookies = HTTPCookieProcessor()
        opener = build_opener(ProxyHandler({}), HTTPSHandler(context=context), cookies)
        def get(url):
            with opener.open(url, timeout=5) as reply:
                return reply.read().decode()
        self.assertIn("Fixture signed out", get(instance.url + "/"))
        self.assertIn("Fixture home Work", get(instance.url + "/sign-in/Work"))
        cookie = next(c for c in cookies.cookiejar if c.name == "fixture_session")
        self.assertTrue(cookie.secure and cookie.has_nonstandard_attr("HttpOnly"))
        self.assertFalse(cookie.discard, "Cold profile persistence needs an explicitly durable mock cookie")
        body = get(editor.url + "/editor")
        self.assertIn("Notes editor", body)
        self.assertFalse(editor.events[-1]["cookiePresent"])
        self.assertTrue(instance.events[-1]["user"] == "Work")
        self.assertNotIn(instance.tokens["Work"], json.dumps(instance.events + editor.events))
        self.assertEqual(context.check_hostname, True)
        script = re.findall(r"<script>(.*?)</script>", body)[-1]
        # Run the actual page-owned observation against isolated JS globals.
        # This control cannot qualify WKWebView, but catches a missing raw
        # message-handler check without ever invoking a native operation.
        probe = """
          import assert from 'node:assert/strict';
          import vm from 'node:vm';
          const script = SCRIPT;
          for (const [window, allowed] of [
            [{},true], [{__TAURI_INTERNALS__:{}},false], [{__TAURI__:{}},false],
            [{ipc:{}},false], [{webkit:{messageHandlers:{ipc:{}}}},false],
            [{webkit:{messageHandlers:{unrelated:{}}}},true]
          ]) {
            const node = {textContent:''};
            vm.runInNewContext(script,{window,document:{getElementById:()=>node}});
            assert.equal(node.textContent,allowed?'Remote has no App IPC':'Unexpected App IPC');
          }
        """.replace("SCRIPT", json.dumps(script))
        checked = subprocess.run(["node", "--input-type=module", "-e", probe],
                                 capture_output=True, text=True, timeout=5)
        self.assertEqual(checked.returncode, 0, checked.stderr)

    def handler(self, writer):
        handler = Handler.__new__(Handler)
        handler.server = types.SimpleNamespace(kind="editor", tokens={}, events=[])
        handler.path, handler.headers = "/editor", {}
        handler.send_response = lambda _: None
        handler.send_header = lambda *_: None
        handler.end_headers = lambda: None
        handler.wfile = writer
        return handler

    def test_response_write_return_is_separate_from_receipt_and_does_not_claim_a_native_commit(self):
        writer = io.BytesIO()
        handler = self.handler(writer)
        handler.do_GET()
        self.assertEqual(len(handler.server.events), 1)
        event = handler.server.events[0]["response"]
        self.assertEqual(event["state"], "written")
        self.assertEqual(event["status"], 200)
        self.assertEqual(event["writtenBytes"], len(writer.getvalue()))
        self.assertEqual(event["writtenBytes"], event["bodyBytes"])
        self.assertGreaterEqual(event["writeElapsedMs"], event["responseElapsedMs"])
        self.assertIsInstance(event["receivedUnixMs"], int)
        self.assertIsInstance(event["writeReturnedUnixMs"], int)
        self.assertIn(b"Notes editor", writer.getvalue())
        self.assertNotIn("passed", event)

    def test_response_transport_failure_updates_the_same_request_without_recording_error_text(self):
        for error, kind in ((BrokenPipeError("private transport detail"), "broken-pipe"),
                            (ConnectionResetError("private transport detail"), "connection-reset"),
                            (ssl.SSLError("private transport detail"), "tls")):
            with self.subTest(kind=kind):
                handler = self.handler(types.SimpleNamespace(write=lambda _: (_ for _ in ()).throw(error)))
                handler.do_GET()
                self.assertEqual(len(handler.server.events), 1, "Do not inflate the navigation oracle")
                event = handler.server.events[0]["response"]
                self.assertEqual(event["state"], "transport-failed")
                self.assertEqual(event["transportFailure"], kind)
                self.assertEqual(event["status"], 200)
                self.assertNotIn("writtenBytes", event)
                self.assertNotIn("private transport detail", json.dumps(event))

    def test_published_response_snapshot_stays_stable_when_a_write_finishes(self):
        snapshots = []
        def write(data):
            event = handler.server.events[0]
            snapshots.append((event["response"], json.dumps(handler.server.events), set(event)))
            return len(data)
        handler = self.handler(types.SimpleNamespace(write=write))
        handler.do_GET()
        during, serialized, keys = snapshots[0]
        event = handler.server.events[0]
        self.assertEqual(during["state"], "writing")
        self.assertNotIn("writtenBytes", during)
        self.assertEqual(json.loads(serialized)[0]["response"], during)
        self.assertEqual(set(event), keys, "A request worker must not resize the published request")
        self.assertEqual(event["response"]["state"], "written")
        self.assertIsNot(event["response"], during)


class TLSTransportControls(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        parent = Path(os.environ.get("TMPDIR", "/root/dev/tmp")).resolve()
        if parent.is_relative_to(runner.ROOT):
            raise RuntimeError("Private mock material belongs outside the repository")
        root = Path(cls.enterClassContext(tempfile.TemporaryDirectory(dir=parent)))
        cls.ca, cls.cert, cls.key = runtime.certificate(root, localhost=True)

    def setUp(self):
        self.server = Site(self.cert, self.key, "editor")
        self.addCleanup(self.server.stop)
        self.server.start()
        self.address = ("127.0.0.1", self.server.server_port)

    def test_unfinished_tls_does_not_hold_two_verified_editor_requests(self):
        context = ssl.create_default_context(cafile=str(self.ca))
        opener = build_opener(ProxyHandler({}), HTTPSHandler(context=context))
        with socket.create_connection(self.address, timeout=1):
            # Keep the first connection open without TLS while two ordinary
            # verified requests use at most one additional connection at a time.
            for _ in range(2):
                with opener.open(self.server.url + "/editor", timeout=1) as reply:
                    self.assertEqual(reply.status, 200)
                    self.assertIn(b"<input id='draft'", reply.read())
            self.assertEqual(len(self.server.events), 2)
        # A client read can finish before its worker publishes the write snapshot.
        self.server.stop()
        self.assertTrue(all(e["response"]["state"] == "written" for e in self.server.events))
        self.assertTrue(context.check_hostname)
        self.assertEqual(context.verify_mode, ssl.CERT_REQUIRED)

    def test_idle_tls_is_closed_by_the_connection_deadline_without_a_request_event(self):
        self.server.connection_timeout = 0.15
        with socket.create_connection(self.address, timeout=1) as pending:
            self.assertEqual(pending.recv(1), b"")
        self.assertEqual(self.server.events, [])

    def test_stop_joins_an_unfinished_tls_worker_and_closes_its_socket(self):
        self.server.connection_timeout = 1
        entered = threading.Event()
        exited = threading.Event()
        workers = []
        original = self.server.process_request_thread
        def observe(*args):
            workers.append(threading.current_thread())
            entered.set()
            try:
                return original(*args)
            finally:
                exited.set()
        with patch.object(self.server, "process_request_thread", observe):
            with socket.create_connection(self.address, timeout=1) as pending:
                self.assertTrue(entered.wait(0.5), "TLS was not dispatched to its own worker")
                self.server.stop()
                self.assertTrue(exited.is_set(), "stop returned before the owned TLS worker ended")
                self.assertTrue(all(not worker.is_alive() for worker in workers))
                self.assertFalse(self.server.thread.is_alive())
                self.assertEqual(self.server.socket.fileno(), -1)
                self.assertEqual(pending.recv(1), b"")
        self.assertEqual(self.server.events, [])


if __name__ == "__main__":
    unittest.main()
