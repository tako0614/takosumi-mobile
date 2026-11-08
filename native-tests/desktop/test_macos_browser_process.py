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
import macos_shutdown_sample as sampler


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        directory = self.enterContext(tempfile.TemporaryDirectory(prefix="m3-images-"))
        self.root = Path(directory)
        self.browser, self.guard = self.root / "browser", self.root / "guard"
        self.browser.write_bytes(b"browser")
        self.guard.write_bytes(b"guard")
        self.browser_image = self.enterContext(runner.PinnedImage(self.browser))
        self.guard_image = self.enterContext(runner.PinnedImage(self.guard))

    def test_image_alias_accepts_hardlink_but_not_copy_replacement_or_missing(self):
        alias = self.root / "clone" / "browser"
        alias.parent.mkdir()
        alias.hardlink_to(self.browser)
        self.assertTrue(self.browser_image.matches(alias))
        self.browser.unlink()
        self.browser.write_bytes(b"browser")
        self.assertFalse(self.browser_image.matches(self.browser))
        self.assertTrue(self.browser_image.matches(alias))
        alias.unlink()
        alias.write_bytes(b"browser")
        self.assertFalse(self.browser_image.matches(alias))
        alias.unlink()
        self.assertFalse(self.browser_image.matches(alias))
        self.assertFalse(self.browser_image.matches(self.root))
        with self.assertRaises(OSError):
            runner.PinnedImage(alias)
        with self.assertRaises(OSError):
            runner.PinnedImage(self.root)

    def test_attach_accepts_hardlink_alias_with_stable_process_before_cdp(self):
        alias = self.root / "clone-browser"
        alias.hardlink_to(self.browser)
        processes = mock.Mock()
        identities = {11: {"pid": 11, "parent": 10, "group": 11, "start": 100},
                      12: {"pid": 12, "parent": 11, "group": 11, "start": 101}}
        processes.facts.side_effect = identities.get
        processes.image.side_effect = {11: self.guard, 12: alias}.get
        (self.root / "inspection.json").write_text(json.dumps({"pid": 12, "port": 9222}))
        observer = runner.MacObserver(self.root, "excalidraw", "", processes, self.browser_image, self.guard_image)
        observer.app_pid = 10
        class ReachedCdp(Exception):
            pass
        with mock.patch.object(runner, "listeners", return_value=[12]), \
                mock.patch.object(runner.http.client, "HTTPConnection", side_effect=ReachedCdp) as connection, \
                self.assertRaises(ReachedCdp):
            observer.attach({"pid": 11, "opened": {"nativePid": 12}, "directory": str(self.root)})
        connection.assert_called_once_with("127.0.0.1", 9222, timeout=5)

    def test_sampler_thread_failure_still_runs_the_normal_close_body(self):
        with tempfile.TemporaryDirectory(prefix="m3-sample-start-") as directory:
            observer = mock.Mock(identities=[], phase="restart", data=Path(directory), proof={})
            with mock.patch.object(sampler.threading, "Thread") as thread:
                thread.return_value.start.side_effect = RuntimeError("cannot start new thread")
                entered = False
                with sampler.watch_shutdowns(observer, []):
                    entered = True
                # Partial and final quit share a phase: the second watcher
                # must preserve the first watcher's diagnostics on disk.
                with sampler.watch_shutdowns(observer, []):
                    pass
                self.assertTrue(entered)
                thread.return_value.join.assert_not_called()
            self.assertEqual(observer.proof["shutdownSamples"][0]["status"], "thread-start-error")
            saved = json.loads((Path(directory) / "shutdown-samples-restart.json").read_text())
            self.assertEqual(len(saved), 2)
            self.assertEqual(saved, observer.proof["shutdownSamples"])

    def test_shutdown_watch_ignores_old_birth_and_completed_shutdown(self):
        identity = {"pid": 12, "start": 101}
        def event(kind, start=101):
            return sampler.PREFIX + json.dumps(dict(identity, start=start, event=kind)) + "\n"
        with tempfile.TemporaryDirectory(prefix="m3-shutdown-") as directory:
            log = Path(directory) / "browser.log"
            log.write_text(event("cef-shutdown-begin", 100))
            self.assertFalse(sampler.shutdown_pending(log, identity))
            with log.open("a") as output:
                output.write(event("cef-shutdown-begin") + sampler.PREFIX + "{broken\n" + sampler.PREFIX + "[]\n")
            self.assertTrue(sampler.shutdown_pending(log, identity))
            with log.open("a") as output:
                output.write(event("cef-shutdown-return"))
            self.assertFalse(sampler.shutdown_pending(log, identity))

    def test_stack_sample_refuses_changed_identity_before_or_after_sampling(self):
        child = {"pid": 12, "parent": 11, "start": 101}
        parent = {"pid": 11, "parent": 10, "start": 100}
        browser, guard = self.browser, self.guard
        with tempfile.TemporaryDirectory(prefix="m3-sample-") as directory:
            for mutation in ("birth", "parent", "guard", "image", "after"):
                with self.subTest(mutation=mutation):
                    facts = {11: dict(parent), 12: dict(child)}
                    if mutation == "birth":
                        facts[12]["start"] += 1
                    if mutation == "parent":
                        facts[12]["parent"] = 99
                    if mutation == "guard":
                        facts[11]["start"] += 1
                    processes = mock.Mock()
                    processes.facts.side_effect = facts.get
                    processes.image.side_effect = lambda pid: (Path("/foreign") if mutation == "image"
                                                                else {11: guard, 12: browser}[pid])
                    def sample(args, **options):
                        Path(args[-1]).write_text("untrusted old process sample")
                        facts[12] = dict(child, start=102)
                        return mock.Mock(returncode=0)
                    output = Path(directory) / f"{mutation}.log"
                    with mock.patch.object(sampler.subprocess, "run", side_effect=sample) as command:
                        result = sampler.sample_owned(processes, child, parent, self.browser_image, self.guard_image, output)
                    self.assertEqual(result["status"], f"identity-changed-{'after' if mutation == 'after' else 'before'}")
                    self.assertFalse(output.exists())
                    self.assertEqual(command.call_count, int(mutation == "after"))

    def test_stack_sample_bounds_output_and_uses_owned_subprocess_timeout(self):
        child, parent = {"pid": 12, "parent": 11, "start": 101}, {"pid": 11, "start": 100}
        browser, guard = self.browser, self.guard
        processes = mock.Mock()
        processes.facts.side_effect = {11: parent, 12: child}.get
        processes.image.side_effect = {11: guard, 12: browser}.get
        with tempfile.TemporaryDirectory(prefix="m3-sample-bounds-") as directory:
            output = Path(directory) / "sample.log"
            def sample(args, **options):
                self.assertEqual(args[:4], ["/usr/bin/sample", "12", "1", "100"])
                self.assertEqual(options["timeout"], 3)
                Path(args[-1]).write_bytes(b"x" * 33)
                return mock.Mock(returncode=0)
            with mock.patch.object(sampler, "LIMIT", 32), \
                    mock.patch.object(sampler.subprocess, "run", side_effect=sample):
                result = sampler.sample_owned(processes, child, parent, self.browser_image, self.guard_image, output)
            self.assertEqual((result["status"], result["truncated"], output.stat().st_size), ("captured", True, 32))
            with mock.patch.object(sampler.subprocess, "run", side_effect=sampler.subprocess.TimeoutExpired("sample", 3)):
                result = sampler.sample_owned(processes, child, parent, self.browser_image, self.guard_image, Path(directory) / "missing.log")
            self.assertEqual((result["status"], result["errorType"]), ("sample-error", "TimeoutExpired"))

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
        browser, guard = self.browser, self.guard
        identities = {11: {"pid": 11, "parent": 10, "group": 11, "start": 100},
                      12: {"pid": 12, "parent": 11, "group": 11, "start": 101}}
        with tempfile.TemporaryDirectory(prefix="m3-ownership-") as directory:
            data = Path(directory)
            (data / "inspection.json").write_text(json.dumps({"pid": 12, "port": 9222}))
            process = {"pid": 11, "opened": {"nativePid": 12}, "directory": directory}
            for mutation in ("guard-parent", "browser-parent", "birth", "image", "listener", "reuse", "guard-reuse"):
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
                        if reads > 2 and ((mutation == "reuse" and pid == 12) or (mutation == "guard-reuse" and pid == 11)):
                            return dict(facts[pid], start=102)
                        return facts[pid]

                    processes.facts.side_effect = read
                    processes.image.side_effect = lambda pid: (
                        Path("/foreign/browser") if mutation == "image" else {11: guard, 12: browser}[pid])
                    observer = runner.MacObserver(data, "excalidraw", "", processes, self.browser_image, self.guard_image)
                    observer.app_pid = 10
                    with mock.patch.object(runner, "listeners", return_value=[99] if mutation == "listener" else [12]), \
                            mock.patch.object(runner.http.client, "HTTPConnection") as connection, \
                            self.assertRaises(runner.Failure):
                        observer.attach(process)
                    connection.assert_not_called()


if __name__ == "__main__":
    unittest.main()
