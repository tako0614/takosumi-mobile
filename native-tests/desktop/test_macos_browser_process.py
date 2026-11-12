#!/usr/bin/env python3
"""Portable rejection cases for M3's process/listener and cleanup evidence.

No native process, browser or site is started. Native Mac acceptance is separate.
"""
import copy
import json
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace
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

    def test_packaged_executables_must_be_inside_the_apps_own_resources(self):
        binary = Path("/Applications/Takosumi.app/Contents/MacOS/takosumi-mobile")
        browser, guard = runner.packaged_files(binary)
        cef = binary.parent.parent / "Resources/cef"
        self.assertEqual(browser, cef / "takosumi-cef-browser-process.app/Contents/MacOS/takosumi-cef-browser-process")
        self.assertEqual(guard, cef / "takosumi-browser-guard")
        for invalid in (Path("/tmp/takosumi-mobile"), Path("/tmp/package/Contents/MacOS/app"),
                        Path("/Applications/Takosumi.app/MacOS/app")):
            with self.assertRaises(runner.Failure):
                runner.packaged_files(invalid)

    def test_packaged_environment_cannot_use_inherited_executable_or_runtime_overrides(self):
        args = SimpleNamespace(site="photopea", packaged=True, browser=Path("/explicit/browser"),
                               guard=Path("/explicit/guard"))
        base = {"PATH": "/usr/bin", "TAKOSUMI_BROWSER_PROCESS_PROGRAM": "/foreign/browser",
                "TAKOSUMI_BROWSER_PROCESS_GUARD": "/foreign/guard",
                "TAKOSUMI_BROWSER_PROCESS_RUNTIME": "/foreign/runtime",
                "TAKOSUMI_BROWSER_PROCESS_PHASE": "foreign"}
        env = runner.browser_env(base, args, self.root, self.root / "rt", "restart")
        for key in ("PROGRAM", "GUARD", "RUNTIME"):
            self.assertNotIn("TAKOSUMI_BROWSER_PROCESS_" + key, env)
        self.assertEqual(env["PATH"], base["PATH"])
        self.assertEqual(env["TAKOSUMI_BROWSER_PROCESS_PHASE"], "restart")
        args.packaged = False
        env = runner.browser_env(base, args, self.root, self.root / "rt", "")
        self.assertEqual(env["TAKOSUMI_BROWSER_PROCESS_PROGRAM"], str(args.browser))
        self.assertEqual(env["TAKOSUMI_BROWSER_PROCESS_GUARD"], str(args.guard))
        self.assertEqual(env["TAKOSUMI_BROWSER_PROCESS_RUNTIME"], str(self.root / "rt"))
        self.assertNotIn("TAKOSUMI_BROWSER_PROCESS_PHASE", env)

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
            observer = mock.Mock(identities=[], phase="restart", data=Path(directory), proof={}, sample_shutdowns=True)
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

    def test_normal_close_does_not_start_a_sampler_without_explicit_opt_in(self):
        observer = self.failed_observer()
        self.assertIs(observer.proof["shutdownSampling"], False)
        with mock.patch.object(sampler.threading, "Thread") as thread, \
                mock.patch.object(sampler, "sample_owned") as sample:
            with sampler.watch_shutdowns(observer, []):
                observer.proof["normalCloseRan"] = True
        self.assertTrue(observer.proof["normalCloseRan"])
        thread.assert_not_called()
        sample.assert_not_called()

    def test_opt_in_shutdown_watch_still_samples_the_registered_browser_once(self):
        observer = self.failed_observer(sample_shutdowns=True)
        self.assertIs(observer.proof["shutdownSampling"], True)
        ready = sampler.threading.Event()
        page = SimpleNamespace(native={"nativePid": 12}, process={"pid": 11, "directory": str(self.root)})
        def capture(*args):
            ready.set()
            return {"pid": 12, "diagnosticOnly": True, "status": "captured"}
        with mock.patch.object(sampler, "shutdown_pending", return_value=True), \
                mock.patch.object(sampler.time, "monotonic", side_effect=[0, 0, 3, 3]), \
                mock.patch.object(sampler, "sample_owned", side_effect=capture) as sample:
            with sampler.watch_shutdowns(observer, [page]):
                self.assertTrue(ready.wait(2), "Opt-in watcher did not reach its registered target")
        sample.assert_called_once()
        self.assertEqual(sample.call_args.args[1:3], tuple(observer.identities[:2][::-1]))
        self.assertEqual(observer.proof["shutdownSamples"], [{"pid": 12, "diagnosticOnly": True, "status": "captured"}])

    def test_normal_close_preserves_its_body_failure_without_starting_a_sampler(self):
        observer = self.failed_observer()
        failure = runner.Failure("original close failure")
        with mock.patch.object(sampler.threading, "Thread") as thread:
            try:
                with sampler.watch_shutdowns(observer, []):
                    raise failure
            except runner.Failure as caught:
                self.assertIs(caught, failure)
            else:
                self.fail("Original close failure was hidden")
        thread.assert_not_called()

    def failed_observer(self, sample_shutdowns=False):
        observer = runner.MacObserver(self.root, "photopea", "", mock.Mock(),
                                      self.browser_image, self.guard_image, sample_shutdowns=sample_shutdowns)
        observer.identities = [
            {"pid": 11, "parent": 10, "start": 100}, {"pid": 12, "parent": 11, "start": 101},
            {"pid": 21, "parent": 10, "start": 102}, {"pid": 22, "parent": 21, "start": 103}]
        observer.proof["opened"] = {"processes": [
            {"pid": 11, "opened": {"nativePid": 12}}, {"pid": 21, "opened": {"nativePid": 22}}]}
        observer.first_launch = mock.Mock(side_effect=runner.Failure("original flow failed"))
        return observer

    def test_failed_flow_collects_owned_stacks_without_replacing_the_error(self):
        observer = self.failed_observer()
        observer.identities.extend([
            {"pid": 31, "parent": 10, "start": 104}, {"pid": 32, "parent": 31, "start": 105}])
        observer.proof["opened"]["processes"].append({"pid": 31, "opened": {"nativePid": 32}})
        def collect(processes, child, parent, browser, guard, output):
            self.assertEqual(observer.proof["error"], "Failure: original flow failed")
            self.assertEqual(child["parent"], parent["pid"])
            self.assertEqual((browser, guard), (self.browser_image, self.guard_image))
            self.assertEqual(output, self.root / "evidence/failure-first" / f"sample-{child['pid']}.log")
            return {"pid": child["pid"], "diagnosticOnly": True, "status": "captured"}
        with mock.patch.object(runner, "sample_owned", side_effect=collect) as capture, \
                self.assertRaisesRegex(runner.Failure, "original flow failed"):
            observer.run()
        self.assertEqual(capture.call_count, 2)
        saved = json.loads((self.root / "browser-observation.json").read_text())
        self.assertEqual([x["pid"] for x in saved["failureSamples"]], [12, 22])
        self.assertFalse(saved["passed"])
        self.assertEqual(saved["error"], "Failure: original flow failed")

    def test_failure_stack_capture_requires_identity_and_does_not_repeat_shutdown_sample(self):
        observer = self.failed_observer()
        observer.proof["shutdownSamples"] = [{"pid": 12, "status": "sample-error"}]
        observer.identities = observer.identities[:2]
        with mock.patch.object(runner, "sample_owned") as capture, \
                self.assertRaisesRegex(runner.Failure, "original flow failed"):
            observer.run()
        capture.assert_not_called()
        self.assertEqual(observer.proof["failureSamples"], [])

    def test_failure_stack_error_cannot_mask_the_original_flow_failure(self):
        observer = self.failed_observer()
        with mock.patch.object(runner, "sample_owned", side_effect=OSError("private diagnostic detail")), \
                self.assertRaisesRegex(runner.Failure, "original flow failed"):
            observer.run()
        saved = json.loads((self.root / "browser-observation.json").read_text())
        self.assertEqual([x["errorType"] for x in saved["failureSamples"]], ["OSError", "OSError"])
        self.assertNotIn("private diagnostic detail", json.dumps(saved))

    def test_successful_flow_does_not_collect_failure_stacks(self):
        observer = self.failed_observer()
        observer.first_launch.side_effect = None
        with mock.patch.object(runner, "sample_owned") as capture:
            observer.run()
        capture.assert_not_called()
        self.assertNotIn("failureSamples", observer.proof)

    def test_system_quit_requires_same_owned_identity_and_exact_request_receipt(self):
        child = {"pid": 12, "parent": 11, "group": 11, "start": 101}
        parent = {"pid": 11, "parent": 10, "group": 11, "start": 100}
        processes = mock.Mock()
        processes.facts.side_effect = {11: parent, 12: child}.get
        processes.image.side_effect = {11: self.guard, 12: self.browser}.get
        observer = runner.MacObserver(self.root, "excalidraw", "", processes,
                                      self.browser_image, self.guard_image, self.root / "normal-system-quit")
        observer.identities = [parent, child]
        page = mock.Mock(native={"nativePid": 12}, process={"pid": 11})
        reply = mock.Mock(returncode=0, stdout='{"requested":true,"pid":12}')
        with mock.patch.object(runner.subprocess, "run", return_value=reply) as command:
            observer.system_quit(page)
        self.assertEqual(command.call_args.args[0], [str(self.root / "normal-system-quit"), "12", "11", "101", "100"])
        self.assertEqual(observer.proof["systemQuitRequests"], [{"pid": 12, "start": 101, "requested": True, "completionProven": False}])
        for valid in [(False, False), (True, False)]:
            with mock.patch.object(runner, "same_owner", side_effect=valid), \
                    mock.patch.object(runner.subprocess, "run", return_value=reply) as command, \
                    self.assertRaises(runner.Failure):
                observer.system_quit(page)
            self.assertEqual(command.call_count, int(valid[0]))
        for bad in [mock.Mock(returncode=4, stdout=''), mock.Mock(returncode=0, stdout='{"requested":false,"pid":12}'),
                    mock.Mock(returncode=0, stdout='{"requested":true,"pid":13}')]:
            with mock.patch.object(runner.subprocess, "run", return_value=bad), self.assertRaises(runner.Failure):
                observer.system_quit(page)
        self.assertEqual(len(observer.proof["systemQuitRequests"]), 1)

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
                        # Only the sampler itself is the sampled-process probe;
                        # kernel-side state probes run first under the same
                        # identity gate and must not touch args[-1] as a file.
                        if args[0] == "/usr/bin/sample":
                            Path(args[-1]).write_text("untrusted old process sample")
                            facts[12] = dict(child, start=102)
                        return mock.Mock(returncode=0, stdout=b"")
                    output = Path(directory) / f"{mutation}.log"
                    with mock.patch.object(sampler.subprocess, "run", side_effect=sample) as command:
                        result = sampler.sample_owned(processes, child, parent, self.browser_image, self.guard_image, output)
                    self.assertEqual(result["status"], f"identity-changed-{'after' if mutation == 'after' else 'before'}")
                    self.assertFalse(output.exists())
                    self.assertFalse(output.with_suffix(".stderr.log").exists())
                    self.assertEqual(output.with_suffix(".state.log").exists(), mutation == "after")
                    self.assertEqual(command.call_count, 4 if mutation == "after" else 0)

    def test_stack_sample_bounds_output_and_uses_owned_subprocess_timeout(self):
        child, parent = {"pid": 12, "parent": 11, "start": 101}, {"pid": 11, "start": 100}
        browser, guard = self.browser, self.guard
        processes = mock.Mock()
        processes.facts.side_effect = {11: parent, 12: child}.get
        processes.image.side_effect = {11: guard, 12: browser}.get
        with tempfile.TemporaryDirectory(prefix="m3-sample-bounds-") as directory:
            output = Path(directory) / "sample.log"
            def sample(args, **options):
                if args[0] != "/usr/bin/sample":
                    return mock.Mock(returncode=0, stdout=b"")
                self.assertEqual(args[:4], ["/usr/bin/sample", "12", "1", "100"])
                self.assertEqual(args[4:6], ["-mayDie", "-file"])
                self.assertEqual(options["timeout"], 7)
                Path(args[-1]).write_bytes(b"x" * 33)
                return mock.Mock(returncode=0)
            with mock.patch.object(sampler, "LIMIT", 32), \
                    mock.patch.object(sampler.subprocess, "run", side_effect=sample):
                result = sampler.sample_owned(processes, child, parent, self.browser_image, self.guard_image, output)
            self.assertEqual((result["status"], result["truncated"], output.stat().st_size), ("captured", True, 32))
            with mock.patch.object(sampler.subprocess, "run", side_effect=sampler.subprocess.TimeoutExpired("sample", 5)):
                result = sampler.sample_owned(processes, child, parent, self.browser_image, self.guard_image, Path(directory) / "missing.log")
            self.assertEqual((result["status"], result["errorType"]), ("sample-error", "TimeoutExpired"))

    def test_sampler_timeout_keeps_bounded_diagnostics_without_becoming_a_pass(self):
        child, parent = {"pid": 12, "parent": 11, "start": 101}, {"pid": 11, "start": 100}
        processes = mock.Mock()
        processes.facts.side_effect = {11: parent, 12: child}.get
        processes.image.side_effect = {11: self.guard, 12: self.browser}.get
        with tempfile.TemporaryDirectory(prefix="m3-sample-timeout-") as directory:
            for changed in (False, True):
                with self.subTest(changed_identity=changed):
                    output = Path(directory) / f"sample-{changed}.log"
                    def sample(args, **options):
                        if args[0] != "/usr/bin/sample":
                            return mock.Mock(returncode=0, stdout=b"")
                        self.assertEqual(options["timeout"], 7)
                        Path(args[-1]).write_bytes(b"s" * 33)
                        options["stderr"].write(b"e" * 17)
                        if changed:
                            processes.facts.side_effect = {11: parent, 12: dict(child, start=102)}.get
                        raise sampler.subprocess.TimeoutExpired("sample", 5)
                    with mock.patch.object(sampler, "LIMIT", 32), \
                            mock.patch.object(sampler, "STDERR_LIMIT", 16), \
                            mock.patch.object(sampler.subprocess, "run", side_effect=sample):
                        result = sampler.sample_owned(processes, child, parent, self.browser_image, self.guard_image, output)
                    if changed:
                        self.assertEqual(result["status"], "identity-changed-after")
                        self.assertFalse(output.exists())
                        self.assertFalse(output.with_suffix(".stderr.log").exists())
                    else:
                        self.assertEqual((result["status"], result["errorType"]), ("sample-error", "TimeoutExpired"))
                        self.assertTrue(result["diagnosticOnly"])
                        self.assertEqual(result["partialStack"], {"bytes": 32, "truncated": True})
                        self.assertEqual(result["stderr"], {"bytes": 16, "truncated": True})
                        self.assertEqual(output.read_bytes(), b"s" * 32)
                        self.assertEqual(output.with_suffix(".stderr.log").read_bytes(), b"e" * 16)

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

    def test_busy_dialog_retries_only_while_the_profile_still_runs(self):
        receipt = self.root / "guard.json"
        page = mock.Mock()
        page.process = {"guardReceipt": str(receipt)}
        calls = []

        def dialog(page_arg, mark, kind, what, bound=runner.SETTLE_WAIT):
            calls.append((page_arg, mark, kind, what, bound))
            if "always fail" in what:
                raise runner.Failure(what)
            if len(calls) == 1:
                raise runner.Failure("Personal beforeunload did not open")
            return {"what": what}

        # The real staticmethod reads the receipt path: a missing file means
        # the guard has not ended, so the quit is still pending and the same
        # mark (events arriving in the inter-wait gap are not missed) gets one
        # more bounded window.
        observer = mock.Mock()
        # A plain Mock would mint a truthy real attribute; pin the fixture path.
        observer.real = None
        observer.dialog.side_effect = dialog
        observer.guard_ended = runner.MacObserver.guard_ended
        result = runner.MacObserver.busy_dialog(observer, page, "mark", "beforeunload",
                                                "Personal beforeunload did not open", 30)
        self.assertEqual(result, {"what": "Personal beforeunload did not open after the busy-renderer window"})
        self.assertEqual(len(calls), 2)
        self.assertEqual(calls[1], (page, "mark", "beforeunload",
                                    "Personal beforeunload did not open after the busy-renderer window", 30))

        # A parseable receipt means the profile exited without ever opening
        # the dialog: the quit skipped it, so there is no retry to hide that.
        receipt.write_text("{}")
        calls.clear()
        with self.assertRaises(runner.Failure) as raised:
            runner.MacObserver.busy_dialog(observer, page, "mark", "beforeunload",
                                           "Personal beforeunload did not open", 30)
        self.assertIn("already exited", str(raised.exception))
        self.assertEqual(len(calls), 1)

        # The allowance is exactly one more window: a still-pending quit whose
        # dialog never surfaces fails rather than waiting forever.
        receipt.unlink()
        calls.clear()
        with self.assertRaises(runner.Failure) as raised:
            runner.MacObserver.busy_dialog(observer, page, "mark", "beforeunload",
                                           "Personal beforeunload did not open always fail", 30)
        self.assertIn("always fail after the busy-renderer window", str(raised.exception))
        self.assertEqual(len(calls), 2)

    def test_busy_dialog_real_site_keeps_measuring_while_the_profile_lives(self):
        receipt = self.root / "guard.json"
        page = mock.Mock()
        page.process = {"guardReceipt": str(receipt)}
        calls = []

        def dialog(page_arg, mark, kind, what, bound=runner.SETTLE_WAIT):
            calls.append(what)
            if len(calls) < 4:
                raise runner.Failure(what)
            return {"what": what}

        # A real site gets four extra bounded windows (about 150 s with the
        # first): each retry re-checks the guard, so a late dialog still wins
        # and a finished profile still fails with its real name.
        observer = mock.Mock()
        observer.real = {"origin": "https://example.invalid"}
        observer.dialog.side_effect = dialog
        observer.guard_ended = runner.MacObserver.guard_ended
        result = runner.MacObserver.busy_dialog(observer, page, "mark", "beforeunload",
                                                "Personal beforeunload did not open", 30)
        self.assertEqual(result, {"what": calls[-1]})
        self.assertEqual(len(calls), 4)
        self.assertEqual(calls[1:], ["Personal beforeunload did not open after the busy-renderer window",
                                     "Personal beforeunload did not open after 2 busy-renderer windows",
                                     "Personal beforeunload did not open after 3 busy-renderer windows"])

        # A dialog that never surfaces while the browser stays alive still
        # fails - after all five bounded windows, not on the first starve.
        calls.clear()

        def always_fail(page_arg, mark, kind, what, bound=runner.SETTLE_WAIT):
            calls.append(what)
            raise runner.Failure(what)

        observer.dialog.side_effect = always_fail
        with self.assertRaises(runner.Failure) as raised:
            runner.MacObserver.busy_dialog(observer, page, "mark", "beforeunload",
                                           "Personal beforeunload did not open", 30)
        self.assertIn("after 4 busy-renderer windows", str(raised.exception))
        self.assertEqual(len(calls), 5)

        # The profile ending between windows stops the retry at once.
        receipt.write_text("{}")
        calls.clear()
        with self.assertRaises(runner.Failure) as raised:
            runner.MacObserver.busy_dialog(observer, page, "mark", "beforeunload",
                                           "Personal beforeunload did not open", 30)
        self.assertIn("already exited", str(raised.exception))
        self.assertEqual(len(calls), 1)

    def test_quiet_renderer_waits_for_one_frame_silent_window_then_asks_anyway(self):
        page = mock.Mock()
        page.process = {"profile": "profile-id"}
        marks, events, drains = [0], [], []
        page.cdp.mark.side_effect = lambda: marks.append(len(events)) or marks[-1]
        page.cdp.since.side_effect = lambda mark: events[mark:]
        page.cdp.drain.side_effect = lambda seconds: drains.append(seconds)

        # No frame events at all: the first quiet window qualifies and the
        # proof records the wait honestly.
        observer = mock.Mock()
        observer.proof = {}
        runner.MacObserver.quiet_renderer(observer, page, quiet=0.05, bound=1)
        self.assertEqual(drains, [0.05])
        self.assertEqual(observer.proof["quietRenderer"],
                         [{"profile": "profile-id", "quietSeconds": 0.05,
                           "waitedSeconds": observer.proof["quietRenderer"][0]["waitedSeconds"],
                           "arrived": True}])
        self.assertLess(observer.proof["quietRenderer"][0]["waitedSeconds"], 0.5)

        # Frame churn plus a non-frame event: only frame lifecycle counts as
        # the storm; once it stops the next window qualifies.
        drains.clear()
        events.extend([{"method": "Page.frameAttached"}, {"method": "Page.domContentEventFired"}])
        page.cdp.drain.side_effect = lambda seconds: (drains.append(seconds),
                                                    len(drains) == 1 and events.append({"method": "Page.frameDetached"}))
        observer.proof = {}
        runner.MacObserver.quiet_renderer(observer, page, quiet=0.05, bound=1)
        self.assertEqual(len(drains), 2)
        self.assertTrue(observer.proof["quietRenderer"][0]["arrived"])

        # A storm that never breaks still asks on time: the bound is
        # honored, arrival is recorded false, and nothing raises.
        drains.clear()
        marks.clear()
        marks.append(0)
        events.clear()
        page.cdp.since.side_effect = lambda mark: [{"method": "Page.frameAttached"}]
        page.cdp.drain.side_effect = lambda seconds: drains.append(seconds)
        observer.proof = {}
        started = time.monotonic()
        runner.MacObserver.quiet_renderer(observer, page, quiet=0.02, bound=0.15)
        self.assertLess(time.monotonic() - started, 1)
        self.assertEqual(observer.proof["quietRenderer"][0]["arrived"], False)

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
