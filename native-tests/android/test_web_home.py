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
        built = subprocess.run([compiler, "--release", "17", "-encoding", "UTF-8", "-d", str(classes),
                                str(helper), str(control)], capture_output=True, text=True, timeout=15)
        self.assertEqual(built.returncode, 0, built.stderr)
        checked = subprocess.run([java, "-cp", str(classes), "com.takosumi.fixture.webhome.ConnectionLabelTest"],
                                 capture_output=True, text=True, timeout=10)
        self.assertEqual(checked.returncode, 0, checked.stderr)
        self.assertRegex(checked.stdout, r"PASS: saved connection label checks=[1-9][0-9]*")

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
        self.assertEqual(emulator.owned.call_count, 2)
        self.assertEqual([argv for argv, _, _ in calls], [
            [str(emulator.adb), "-s", runner.SERIAL, "shell", "dumpsys", "activity", name]
            for name in ("lastanr", "lastanr-traces")])
        for _, raw, options in calls:
            self.assertEqual(raw.parent, scratch)
            self.assertEqual(options, {"timeout": 10, "read_output": False})
        for name, entry in result["queries"].items():
            saved = out / (name + ".log")
            self.assertTrue(entry["truncated"])
            self.assertGreater(entry["sourceBytes"], entry["keptBytes"])
            self.assertEqual(saved.stat().st_size, entry["keptBytes"])
            self.assertEqual(entry["keptBytes"],
                             (128 if name == "guest-last-anr" else 512) * 1024)
            self.assertEqual(hashlib.sha256(saved.read_bytes()).hexdigest(), entry["sha256"])
        self.assertFalse(any(out.glob("*-raw.log")))
        self.assertNotIn("passed", result)

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
