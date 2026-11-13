"""SDK-free checks of the actual W6 browser evidence copier."""
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import windows_package_evidence as evidence


class Controls(unittest.TestCase):
    def setUp(self):
        self.root = Path(self.enterContext(tempfile.TemporaryDirectory()))
        self.source = self.root / "browser-smoke"
        self.case = self.source / "sumi-cef-smoke-example"
        self.case.mkdir(parents=True)
        self.destination = self.root / "collected"

    def write(self, relative, data):
        path = self.source / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        return path

    def test_only_the_four_diagnostics_are_copied_with_original_bytes(self):
        expected = {}
        for name in evidence.DIAGNOSTICS:
            expected[name] = (name + " 診断\n").encode()
            (self.case / name).write_bytes(expected[name])
        self.write(self.case.name + "/cef-profiles/Default/Session Storage/000003.log", b"\0private DB")
        self.write(self.case.name + "/cef-profiles/Default/summary.json", b'{"private":true}')
        self.write(self.case.name + "/unknown.json", b"private content")
        self.write("foreign/summary.json", b"foreign case")
        kept = evidence.collect(self.source, self.destination)
        self.assertEqual(set(kept), {str(Path(self.case.name) / name) for name in expected})
        for name, data in expected.items():
            self.assertEqual((self.destination / self.case.name / name).read_bytes(), data)
        self.assertEqual(len(list(self.destination.rglob("*.*"))), 4)

    def test_known_names_do_not_allow_binary_or_oversized_files(self):
        for data in (b"MZexecutable", b"\x7fELFlibrary", b"PK\x03\x04archive", b"text\0database", b"\xffinvalid"):
            (self.case / "browser.log").write_bytes(data)
            with self.subTest(data=data):
                self.assertIsNone(evidence.text_bytes(self.case / "browser.log"))
        with patch.object(evidence, "LIMIT", 8):
            (self.case / "browser.log").write_bytes(b"123456789")
            self.assertIsNone(evidence.text_bytes(self.case / "browser.log"))

    def test_linked_root_case_and_leaf_are_refused_without_reading_them(self):
        foreign = self.root / "foreign"
        foreign.mkdir()
        (foreign / "summary.json").write_bytes(b"private")
        linked = self.root / "linked-root"
        linked.symlink_to(foreign, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "Plain browser evidence root"):
            evidence.collect(linked, self.destination)
        (self.source / "sumi-cef-smoke-linked").symlink_to(foreign, target_is_directory=True)
        (self.case / "summary.json").symlink_to(foreign / "summary.json")
        self.assertEqual(evidence.collect(self.source, self.destination), [])

    def test_hardlinks_and_destination_reuse_are_refused(self):
        outside = self.root / "outside.json"
        outside.write_bytes(b"private")
        os.link(outside, self.case / "summary.json")
        self.assertIsNone(evidence.text_bytes(self.case / "summary.json"))
        evidence.collect(self.source, self.destination)
        with self.assertRaises(FileExistsError):
            evidence.collect(self.source, self.destination)


if __name__ == "__main__":
    unittest.main()
