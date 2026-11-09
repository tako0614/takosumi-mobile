"""Harness controls only. These cannot qualify WKDownload or OS export."""
import copy
import http.client
import json
import os
from pathlib import Path
import ssl
import tempfile
import time
import unittest

from run import (certificate, owned_device, require_interrupted_transfers,
                 require_test_pass, select_runtime, verify_exports)
from server import BLOB_BYTES, COOKIE_BYTES, SENTINEL_BYTES, FixtureServer


class HarnessTests(unittest.TestCase):
    def test_runtime_requires_installed_compatible_phone(self):
        runtime = {"identifier": "com.apple.CoreSimulator.SimRuntime.iOS-26-0", "version": "26.0",
                   "isAvailable": True, "supportedDeviceTypes": [{"name": "iPhone 16", "identifier": "phone-16"}]}
        self.assertEqual(select_runtime({"runtimes": [runtime]}), (runtime["identifier"], "phone-16"))
        for patch in ({"isAvailable": False}, {"version": "16.0"}, {"supportedDeviceTypes": []},
                      {"identifier": "com.apple.CoreSimulator.SimRuntime.tvOS-26-0"}):
            with self.assertRaises(RuntimeError):
                select_runtime({"runtimes": [dict(runtime, **patch)]})

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
                       "closedDuringTransfer": True, "closeTimedOut": False}
            (docs / "fixture-receipt.json").write_text(json.dumps(receipt))
            self.assertEqual(len(verify_exports(docs, evidence)), 3)
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
