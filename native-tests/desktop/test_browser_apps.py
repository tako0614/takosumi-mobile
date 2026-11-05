#!/usr/bin/env python3
"""False-success controls for the public app probe, independent of live sites."""

import copy
import json
from pathlib import Path
import shutil
import struct
import tempfile
import unittest
import zlib

from browser_apps import ORIGINS, PROFILES, create_input, validate_report, verify_export


class ReceiptTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="sumi-browser-protocol-")
        self.data = Path(self.temporary.name)
        (self.data / "Downloads").mkdir()
        self.nonce = "33333333-3333-4333-8333-333333333333"
        self.report = {
            "app": "excalidraw", "mode": "operate", "nonce": self.nonce,
            "origin": ORIGINS["excalidraw"], "profileIsolation": True, "operationRequested": True,
            "context": {"origin": ORIGINS["excalidraw"], "secure": True, "tauri": "undefined",
                        "ipc": "undefined", "wryHandler": "undefined"},
            "personalContext": {"origin": ORIGINS["excalidraw"], "secure": True, "tauri": "undefined",
                                "ipc": "undefined", "wryHandler": "undefined"},
            "profiles": {name: {"data": str(self.data / "profiles" / value),
                                "cache": str(self.data / "profiles" / value / "cache")}
                         for name, value in PROFILES.items()},
            "downloads": [{"finished": True, "failed": False,
                           "destination": (self.data / "Downloads" / "result.excalidraw").as_uri()}],
        }
        self.path = self.data / "Downloads" / "result.excalidraw"
        self.path.write_text("{}")

    def tearDown(self):
        self.temporary.cleanup()

    def validate(self, report):
        return validate_report(report, self.data, "excalidraw", "operate", self.nonce)

    def test_missing_native_or_download_evidence_cannot_pass(self):
        for key in ("nonce", "context", "profiles", "downloads", "profileIsolation"):
            bad = copy.deepcopy(self.report)
            bad.pop(key)
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                self.validate(bad)
        for field, value in (("finished", False), ("failed", True), ("destination", None)):
            bad = copy.deepcopy(self.report)
            bad["downloads"][0][field] = value
            with self.subTest(field=field), self.assertRaises(RuntimeError):
                self.validate(bad)

    def test_flag_ignoring_observer_does_not_qualify_an_operation(self):
        bad = copy.deepcopy(self.report)
        bad.update(mode="observe", operationRequested=False)
        with self.assertRaises(RuntimeError):
            self.validate(bad)
        self.assertIsNone(validate_report(bad, self.data, "excalidraw", "observe", self.nonce))

    def test_missing_or_exposed_messaging_cannot_pass_in_either_profile(self):
        for context in ("context", "personalContext"):
            for field in ("tauri", "ipc", "wryHandler"):
                for value in (None, "object", "function"):
                    bad = copy.deepcopy(self.report)
                    if value is None:
                        bad[context].pop(field)
                    else:
                        bad[context][field] = value
                    with self.subTest(context=context, field=field, value=value), self.assertRaises(RuntimeError):
                        self.validate(bad)

    def test_shared_replacement_or_other_site_cannot_pass(self):
        for replacement in (self.data / "profiles" / "shared", self.data / "profiles" / PROFILES["personal"]):
            bad = copy.deepcopy(self.report)
            bad["profiles"]["work"]["data"] = str(replacement)
            with self.assertRaises(RuntimeError):
                self.validate(bad)
        bad = copy.deepcopy(self.report)
        bad["context"]["origin"] = ORIGINS["photopea"]
        with self.assertRaises(RuntimeError):
            self.validate(bad)

    def test_unfinished_multiple_or_foreign_download_cannot_pass(self):
        for destination in ((self.data / "other.excalidraw").as_uri(), "https://example.com/file",
                            "file://foreign" + str(self.path), self.path.as_uri() + "?unexpected=1"):
            bad = copy.deepcopy(self.report)
            bad["downloads"][0]["destination"] = destination
            with self.subTest(destination=destination), self.assertRaises(RuntimeError):
                self.validate(bad)
        bad = copy.deepcopy(self.report)
        bad["downloads"] *= 2
        with self.assertRaises(RuntimeError):
            self.validate(bad)
        self.path.unlink()
        with self.assertRaises(RuntimeError):
            self.validate(self.report)

    def test_real_export_must_preserve_nonce_and_include_the_edit(self):
        source = create_input(self.data, "excalidraw", self.nonce)
        scene = json.loads(source.read_text())
        self.path.write_text(json.dumps(scene))
        with self.assertRaises(RuntimeError):
            verify_export(self.path, "excalidraw", self.nonce)
        scene["elements"][0]["x"] = 81
        self.path.write_text(json.dumps(scene))
        self.assertGreater(verify_export(self.path, "excalidraw", self.nonce)["bytes"], 0)
        scene["elements"][0]["id"] = "unrelated"
        self.path.write_text(json.dumps(scene))
        with self.assertRaises(RuntimeError):
            verify_export(self.path, "excalidraw", self.nonce)

    def test_png_requires_decoded_fixture_pixels(self):
        self.assertIsNotNone(shutil.which("convert"), "Existing PNG decoder capability is required")

        def chunk(kind, data):
            return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

        def png(left, right):
            row = b"\0" + bytes(left) * 64 + bytes(right) * 64
            return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 128, 96, 8, 2, 0, 0, 0))
                    + chunk(b"IDAT", zlib.compress(row * 96)) + chunk(b"IEND", b""))

        self.path.write_bytes(png((255, 0, 0), (0, 0, 255)))
        self.assertGreater(verify_export(self.path, "photopea", self.nonce)["bytes"], 0)
        self.path.write_bytes(png((0, 255, 0), (0, 0, 255)))
        with self.assertRaises(RuntimeError):
            verify_export(self.path, "photopea", self.nonce)


if __name__ == "__main__":
    unittest.main()
