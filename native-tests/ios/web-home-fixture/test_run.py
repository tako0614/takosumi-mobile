"""Harness/protocol controls only; these do not execute or qualify the iOS App."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import re
import ssl
import subprocess
import sys
import tempfile
import types
import unittest
from unittest.mock import patch
from urllib.request import build_opener, HTTPSHandler, HTTPCookieProcessor, ProxyHandler

import run as runner
from harness import runtime
from web_site import Site


class Controls(unittest.TestCase):
    def setUp(self):
        parent = Path(os.environ.get("TMPDIR", "/root/dev/tmp")).resolve()
        self.assertFalse(parent.is_relative_to(runner.ROOT), "Private mock material belongs outside the repository")
        self.root = Path(self.enterContext(tempfile.TemporaryDirectory(dir=parent)))

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


if __name__ == "__main__":
    unittest.main()
