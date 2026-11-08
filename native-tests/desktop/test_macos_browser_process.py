#!/usr/bin/env python3
"""Portable rejection cases for M3's process/listener and cleanup evidence.

No native process, browser or site is started. Native Mac acceptance is separate.
"""
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import macos_browser_process as runner


class EvidenceTests(unittest.TestCase):
    def test_listener_requires_the_exact_loopback_address_and_owner(self):
        self.assertEqual(runner.parse_listeners("p12\nn127.0.0.1:9222\n", 9222), [12])
        self.assertEqual(runner.parse_listeners("p12\nn127.0.0.1:9222\np13\nn127.0.0.1:9222\n", 9222), [12, 13])
        for text in ("n127.0.0.1:9222\n", "pX\nn127.0.0.1:9222\n", "p12\nn*:9222\n",
                     "p12\nn[::1]:9222\n", "p12\nn127.0.0.1:9223\n", "p12\nn10.0.0.1:9222\n"):
            with self.subTest(text=text), self.assertRaises(runner.Failure):
                runner.parse_listeners(text, 9222)

    def test_incomplete_forced_or_failed_cleanup_cannot_pass(self):
        normal = {"rootExit": 0, "complete": True, "kernelNoChildren": True, "forcedCleanup": 0,
                  "remaining": [], "cleanupErrors": [], "error": None}
        runner.normal_receipt(normal)
        for key, value in (("rootExit", -9), ("complete", False), ("kernelNoChildren", False),
                           ("forcedCleanup", 1), ("remaining", [{"pid": 12}]),
                           ("cleanupErrors", ["freeze failed"]), ("error", "unreadable")):
            receipt = dict(normal, **{key: value})
            with self.subTest(key=key), self.assertRaises(runner.Failure):
                runner.normal_receipt(receipt)
        with self.assertRaises(runner.Failure):
            runner.normal_receipt({"rootExit": 0, "complete": True})

    def test_attach_refuses_an_unowned_or_changed_process_before_cdp(self):
        browser, guard = Path("/owned/browser"), Path("/owned/guard")
        identities = {11: {"pid": 11, "parent": 10, "group": 11, "start": 100},
                      12: {"pid": 12, "parent": 11, "group": 11, "start": 101}}
        with tempfile.TemporaryDirectory(prefix="m3-ownership-") as directory:
            data = Path(directory)
            (data / "inspection.json").write_text(json.dumps({"pid": 12, "port": 9222}))
            process = {"pid": 11, "opened": {"nativePid": 12}, "directory": directory}
            for mutation in ("guard-parent", "browser-parent", "birth", "image", "listener", "reuse"):
                with self.subTest(mutation=mutation):
                    facts = copy.deepcopy(identities)
                    if mutation == "guard-parent":
                        facts[11]["parent"] = 999
                    if mutation == "browser-parent":
                        facts[12]["parent"] = 999
                    if mutation == "birth":
                        facts[12]["start"] = 99
                    processes = mock.Mock()
                    reads = 0

                    def read(pid):
                        nonlocal reads
                        reads += 1
                        if mutation == "reuse" and reads > 2 and pid == 12:
                            return dict(facts[pid], start=102)
                        return facts[pid]

                    processes.facts.side_effect = read
                    processes.image.side_effect = lambda pid: (
                        Path("/foreign/browser") if mutation == "image" else {11: guard, 12: browser}[pid])
                    observer = runner.MacObserver(data, "excalidraw", "", processes, browser, guard)
                    observer.app_pid = 10
                    with mock.patch.object(runner, "listeners", return_value=[99] if mutation == "listener" else [12]), \
                            mock.patch.object(runner.http.client, "HTTPConnection") as connection, \
                            self.assertRaises(runner.Failure):
                        observer.attach(process)
                    connection.assert_not_called()


if __name__ == "__main__":
    unittest.main()
