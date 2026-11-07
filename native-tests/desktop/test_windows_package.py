#!/usr/bin/env python3
"""Unit tests for windows_package.py's pure helpers (W6): the silent install
command, the installed App's name, the comparison with the stage manifest
and what remains after an uninstall. Standard library only; runs on Linux
and Windows."""
import sys
import unittest
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


class UninstallTests(unittest.TestCase):
    def test_what_remains_is_the_products_and_the_fixtures_data(self):
        self.assertEqual(package.identified(["com.takosumi", "com.takosumi.fixture.browserprocess.sumi-w6",
                                             "com.takosumix", "Microsoft", "com.other"]),
                         ["com.takosumi", "com.takosumi.fixture.browserprocess.sumi-w6"])
        listing = {"L": ["com.takosumi"], "R": []}
        self.assertEqual(package.remains(["L", "R"], lambda root: listing[str(root)]),
                         [str(Path("L") / "com.takosumi")])


if __name__ == "__main__":
    unittest.main()
