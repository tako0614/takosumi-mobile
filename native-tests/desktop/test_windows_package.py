#!/usr/bin/env python3
"""Unit tests for windows_package.py's pure helpers (W6): the silent install
command, the installed App's name, the comparison with the stage manifest
and what remains after an uninstall. Standard library only; runs on Linux
and Windows."""
import sys
import unittest
from unittest.mock import patch
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import windows_package as package  # noqa: E402
from windows_smoke import Failure  # noqa: E402


class InstallTests(unittest.TestCase):
    def test_the_silent_install_names_its_directory_last_and_unquoted(self):
        self.assertEqual(package.installer_command(Path("D:/a/_temp/setup.exe"), "D:\\a\\_temp\\w6-app"),
                         [str(Path("D:/a/_temp/setup.exe")), "/S", "/D=D:\\a\\_temp\\w6-app"])
        for directory in ("C:\\Program Files\\Takosumi", 'C:\\a"b'):
            with self.assertRaises(Failure):
                package.installer_command("setup.exe", directory)

    def test_the_app_is_the_one_executable_besides_the_uninstaller(self):
        self.assertEqual(package.installed_app(["cef", "Takosumi.exe", "Uninstall.exe", "resources"]),
                         "Takosumi.exe")
        for names in (["cef", "uninstall.exe"], ["a.exe", "b.exe", "uninstall.exe"]):
            with self.assertRaises(Failure):
                package.installed_app(names)

    def test_the_installed_cef_directory_must_hold_exactly_the_stage(self):
        manifest = {"files": [{"path": "libcef.dll", "size": 3, "sha256": "aaa"},
                              {"path": "locales/en-US.pak", "size": 2, "sha256": "bbb"},
                              {"path": "icudtl.dat", "size": 1, "sha256": "ccc"}]}
        sizes = {"libcef.dll": 3, "locales/en-US.pak": 2, "extra.txt": 9}
        hashes = {"libcef.dll": "aaa", "locales/en-US.pak": "changed", "extra.txt": "x"}
        differences = package.stage_differences(manifest, set(sizes), sizes.__getitem__, hashes.__getitem__)
        self.assertEqual(differences, {"missing": ["icudtl.dat"], "different": ["locales/en-US.pak"],
                                       "unexpected": ["extra.txt"]})
        same = package.stage_differences(manifest, {"libcef.dll", "locales/en-US.pak", "icudtl.dat"},
                                         {"libcef.dll": 3, "locales/en-US.pak": 2, "icudtl.dat": 1}.__getitem__,
                                         {"libcef.dll": "aaa", "locales/en-US.pak": "bbb",
                                          "icudtl.dat": "ccc"}.__getitem__)
        self.assertFalse(any(same.values()))


class SmokeTests(unittest.TestCase):
    def test_application_error_keeps_only_a_pid_and_path_matched_summary(self):
        summary = package.application_error_summary([{
            "processId": "0x1234",
            "appPath": r"c:\program files\takosumi\takosumi-mobile.exe",
            "faultingModule": r"C:\Windows\System32\KERNELBASE.dll",
            "exceptionCode": "0xc0000005",
            "faultingOffset": "0x00000000000010AB",
        }], 0x1234, r"C:\Program Files\Takosumi\Takosumi-Mobile.exe")
        self.assertEqual(summary, {
            "available": True,
            "reason": "pid_and_path_matched",
            "faultingModule": "KERNELBASE.dll",
            "exceptionCode": "0xC0000005",
            "faultingOffset": "0x00000000000010AB",
        })
        self.assertNotIn("appPath", summary)
        self.assertNotIn("processId", summary)

    def test_application_error_requires_pid_and_rejects_a_different_executable(self):
        event = {"processId": "4660", "appPath": r"C:\other\app.exe",
                 "faultingModule": r"C:\Windows\other.dll", "exceptionCode": "0xc0000005",
                 "faultingOffset": "0x10"}
        self.assertEqual(package.application_error_summary([event], 0x1235, r"C:\app.exe"),
                         {"available": False, "reason": "no_matching_event"})
        self.assertEqual(package.application_error_summary([event], 4660, r"C:\app.exe"),
                         {"available": False, "reason": "app_path_mismatch"})

    def test_application_error_can_use_pid_when_event_has_no_executable_path(self):
        summary = package.application_error_summary([{
            "processId": "4660", "faultingModule": r"D:/app/runtime.dll",
            "exceptionCode": "c0000005", "faultingOffset": "0x10",
        }], 4660, r"C:\app.exe")
        self.assertEqual(summary, {
            "available": True,
            "reason": "pid_matched_path_unavailable",
            "faultingModule": "runtime.dll",
            "exceptionCode": "0xC0000005",
            "faultingOffset": "0x10",
        })

    def test_application_error_omits_nonprintable_or_oversized_module_names(self):
        for module in ("bad\nmodule.dll", "x" * 97):
            summary = package.application_error_summary([{
                "processId": "4660", "appPath": r"C:\app.exe", "faultingModule": module,
                "exceptionCode": "0xc0000005", "faultingOffset": "0x10",
            }], 4660, r"C:\app.exe")
            self.assertTrue(summary["available"])
            self.assertIsNone(summary["faultingModule"])

    def test_crash_diagnostic_is_best_effort_and_keeps_the_smoke_failure(self):
        self.assertEqual(package.application_error_summary({"reason": []}, 72, r"C:\app.exe"),
                         {"available": False, "reason": "event_query_unavailable"})

        class Process:
            pid = 72

            def __init__(self, code):
                self.code = code
                # A child can exit after a caller has already formatted an
                # error message. The helper must report the value from poll().
                self.returncode = None

            def poll(self):
                return self.code

        with patch.object(package, "application_error_event",
                          return_value={"available": False, "reason": "no_matching_event"}) as read_event:
            report = {}
            with self.assertRaisesRegex(Failure, "App exited with 3221225477 after 0.6 s up"):
                package.require_app_running(Process(3221225477), report, r"C:\app.exe", None,
                                            "after 0.6 s up")
            read_event.assert_called_once()
            self.assertEqual(report["applicationError"],
                             {"available": False, "reason": "no_matching_event"})

            read_event.reset_mock()
            with self.assertRaisesRegex(Failure, "App exited with 7 before its window appeared"):
                package.require_app_running(Process(7), {}, r"C:\app.exe", None,
                                            "before its window appeared")
            read_event.assert_called_once()

            read_event.reset_mock()
            with self.assertRaisesRegex(Failure, "App exited with 0 after 0.6 s up"):
                package.require_app_running(Process(0), {}, r"C:\app.exe", None,
                                            "after 0.6 s up")
            read_event.assert_not_called()

    def test_processes_left_in_the_apps_job_are_named_in_the_failure(self):
        leftovers = [{"pid": 4120, "image": "msedgewebview2.exe", "parent": 3900,
                      "created": "2026-11-07T11:49:40.123Z"},
                     {"pid": 4188, "image": "msedgewebview2.exe", "parent": 4120, "created": None}]
        self.assertEqual(package.leftover_message({"jobActive": 2, "leftovers": leftovers}),
                         "2 processes were left in the App's job: 4120 msedgewebview2.exe (parent 3900, "
                         "created 2026-11-07T11:49:40.123Z); 4188 msedgewebview2.exe (parent 4120, created ?)")
        self.assertEqual(package.leftover_message({"jobActive": 1, "leftoversError": "denied"}),
                         "1 processes were left in the App's job (not described: denied)")
        self.assertEqual(package.leftover_message({"jobActive": 2}), "2 processes were left in the App's job")


class UninstallTests(unittest.TestCase):
    def test_a_missing_key_or_shortcut_before_the_uninstall_fails(self):
        package.require_installed_entries("Takosumi", True, True)
        for key, shortcut in ((False, True), (True, False), (False, False)):
            with self.assertRaisesRegex(Failure, "vacuously"):
                package.require_installed_entries("TakosumiFixture", key, shortcut)

    def test_what_remains_is_the_products_and_the_fixtures_data(self):
        self.assertEqual(package.identified(["com.takosumi", "com.takosumi.fixture.browserprocess.sumi-w6",
                                             "com.takosumix", "Microsoft", "com.other"]),
                         ["com.takosumi", "com.takosumi.fixture.browserprocess.sumi-w6"])
        listing = {"L": ["com.takosumi"], "R": []}
        self.assertEqual(package.remains(["L", "R"], lambda root: listing[str(root)]),
                         [str(Path("L") / "com.takosumi")])


if __name__ == "__main__":
    unittest.main()
