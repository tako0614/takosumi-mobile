#!/usr/bin/env python3
"""Portable policy and ownership tests for the Windows native fault probe."""
import json
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import windows_native_fault_probe as probe  # noqa: E402


class EventPolicyTests(unittest.TestCase):
    def test_only_initial_breakpoint_is_handled_and_other_exceptions_fail_through(self):
        pid = 73
        created = probe.DebugEvent(probe.DEBUG_EVENT_CREATE_PROCESS, pid, 1)
        self.assertEqual(probe.continue_status(created, pid, False, False),
                         (probe.DBG_CONTINUE, False))

        initial = probe.DebugEvent(probe.DEBUG_EVENT_EXCEPTION, pid, 1,
                                   probe.EXCEPTION_BREAKPOINT, True)
        self.assertEqual(probe.continue_status(initial, pid, True, False),
                         (probe.DBG_CONTINUE, True))
        self.assertEqual(probe.continue_status(initial, pid, True, True),
                         (probe.DBG_EXCEPTION_NOT_HANDLED, True))

        first_chance_av = probe.DebugEvent(probe.DEBUG_EVENT_EXCEPTION, pid, 1,
                                           probe.EXCEPTION_ACCESS_VIOLATION, True)
        last_chance_av = probe.DebugEvent(probe.DEBUG_EVENT_EXCEPTION, pid, 1,
                                          probe.EXCEPTION_ACCESS_VIOLATION, False)
        foreign_av = probe.DebugEvent(probe.DEBUG_EVENT_EXCEPTION, pid + 1, 1,
                                      probe.EXCEPTION_ACCESS_VIOLATION, False)
        self.assertEqual(probe.continue_status(first_chance_av, pid, True, True)[0],
                         probe.DBG_EXCEPTION_NOT_HANDLED)
        self.assertEqual(probe.continue_status(foreign_av, pid, True, True)[0],
                         probe.DBG_EXCEPTION_NOT_HANDLED)
        self.assertFalse(probe.observed_fault(first_chance_av, pid))
        self.assertFalse(probe.observed_fault(foreign_av, pid))
        self.assertTrue(probe.observed_fault(last_chance_av, pid))

    def test_module_projection_keeps_only_basename_and_relative_offset(self):
        result = probe.module_projection("WebView2Loader.dll", 0x1000, 0x19ab)
        self.assertEqual(result, {"faultingModule": "WebView2Loader.dll",
                                  "faultingOffset": "0x9ab", "moduleReason": "module_mapped"})
        self.assertNotIn("0x19ab", json.dumps(result))
        self.assertNotIn("C:\\Windows", json.dumps(result))
        self.assertEqual(probe.module_projection(r"C:\\private\\secret.dll", 0x1000, 0x1100),
                         {"faultingModule": None, "faultingOffset": None,
                          "moduleReason": "module_name_unavailable"})
        self.assertEqual(probe.module_projection("module.dll", 0x2000, 0x1000)["moduleReason"],
                         "module_range_unavailable")


class ProbeLifecycleTests(unittest.TestCase):
    def make_owned_case(self, root):
        case = root / "probe-case"
        case.mkdir()
        (case / ".owned-browser-reference").write_text(case.name, encoding="utf-8")
        (case / "tmp").mkdir()
        binary = root / "takosumi-mobile.exe"
        binary.write_bytes(b"fixture-placeholder")
        return binary, case

    def test_self_launches_owned_fixture_and_records_only_last_chance_summary(self):
        with tempfile.TemporaryDirectory() as temporary:
            binary, case = self.make_owned_case(Path(temporary))
            pid = 73
            events = [
                probe.DebugEvent(probe.DEBUG_EVENT_CREATE_PROCESS, pid, 1, close_handles=(10,)),
                probe.DebugEvent(probe.DEBUG_EVENT_EXCEPTION, pid, 1,
                                 probe.EXCEPTION_BREAKPOINT, True),
                probe.DebugEvent(probe.DEBUG_EVENT_EXCEPTION, pid, 1,
                                 probe.EXCEPTION_ACCESS_VIOLATION, True, 0x1190),
                probe.DebugEvent(probe.DEBUG_EVENT_EXCEPTION, pid, 1,
                                 probe.EXCEPTION_ACCESS_VIOLATION, False, 0x1190),
                probe.DebugEvent(probe.DEBUG_EVENT_EXIT_PROCESS, pid, 1, exit_code=3221225477),
            ]

            class FakeProcess:
                def __init__(self):
                    self.pid = pid
                    self.wait_calls = 0
                    self.killed = False

                def wait(self, timeout=None):
                    self.wait_calls += 1
                    self.exit_code = 3221225477
                    return 3221225477

                def kill(self):
                    self.killed = True

                def poll(self):
                    return getattr(self, "exit_code", None)

            process = FakeProcess()
            launch = {}

            def popen(command, **kwargs):
                launch.update(command=command, kwargs=kwargs, thread=threading.get_ident())
                return process

            class FakeJob:
                def __init__(self):
                    self.closed = False
                    self.adopt_thread = None

                def adopt(self, actual_pid):
                    self.adopt_thread = threading.get_ident()
                    self.pid = actual_pid

                def close(self):
                    self.closed = True

                def terminate(self):
                    self.terminated = True

            job = FakeJob()

            class FakeApi:
                def __init__(self):
                    self.continues = []
                    self.closed_handles = []
                    self.wait_threads = []
                    self.kernel32 = object()

                def wait_for_debug_event(self, timeout_ms):
                    self.wait_threads.append(threading.get_ident())
                    return events.pop(0) if events else None

                def continue_debug_event(self, event, status):
                    self.continues.append((event, status))

                def close_event_handles(self, event):
                    self.closed_handles.extend(event.close_handles)

                def module_for_address(self, actual_pid, address):
                    if address == 0xA420:
                        return "takosumi-mobile.exe", 0xA000
                    if address == 0xB425:
                        return r"C:\\private\\bad.dll", 0xB000
                    self.lookup = (actual_pid, address)
                    return "WebView2Loader.dll", 0x1000

            api = FakeApi()
            secrets = {"GITHUB_TOKEN": "do-not-forward", "CLOUDFLARE_API_TOKEN": "do-not-forward",
                       "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER": r"C:\\private\\runtime",
                       "TAKOSUMI_BROWSER_PROCESS_SITE": "https://example.invalid/",
                       "TAKOSUMI_BROWSER_PROCESS_REAL_SITE": "photopea",
                       "TAKOSUMI_BROWSER_PROCESS_EXTERNAL_LOG": r"C:\\private\\external.log",
                       "TEMP": r"C:\\outside\\temp", "TMP": r"C:\\outside\\tmp",
                       "SystemRoot": r"C:\\Windows", "LOCALAPPDATA": r"C:\\Users\\fixture\\AppData\\Local"}
            published = []
            def unwind(*_):
                self.assertEqual(published, [{"faultingModule": "WebView2Loader.dll",
                                              "faultingOffset": "0x190", "moduleReason": "module_mapped"}])
                return iter((0xA420, 0xB425))
            with patch.object(probe, "stopped_amd64_frames", side_effect=unwind) as stack:
                report = probe.run_probe(binary, case, timeout_seconds=2, source_env=secrets,
                                         site_url="https://127.0.0.1:44321",
                                         popen_factory=popen, job_factory=lambda: job, api_factory=lambda: api,
                                         on_module_observed=published.append)
            stack.assert_called_once_with(api.kernel32, pid, 1, 0x1190, case.resolve())

            self.assertEqual(report, {"diagnosticComplete": True, "observed": True,
                                     "reason": "last_chance_access_violation",
                                     "exceptionCode": "0xC0000005", "faultingModule": "WebView2Loader.dll",
                                     "faultingOffset": "0x190", "moduleReason": "module_mapped",
                                     "callerFrames": [{"module": "takosumi-mobile.exe", "offset": "0x420"}],
                                     "processExitCode": 3221225477,
                                     "jobHandleClosed": True, "ownedProcessReaped": True})
            self.assertNotIn("passed", report)
            self.assertNotIn("0x1190", json.dumps(report))
            self.assertNotIn("0xa420", json.dumps(report))
            self.assertNotIn("C:\\private", json.dumps(report))
            self.assertNotIn(str(binary), json.dumps(report))
            self.assertNotIn(str(case), json.dumps(report))
            self.assertEqual(launch["command"], [str(binary)])
            self.assertEqual(launch["thread"], job.adopt_thread)
            self.assertEqual(api.wait_threads, [launch["thread"]] * 5)
            self.assertEqual([status for _, status in api.continues], [
                probe.DBG_CONTINUE, probe.DBG_CONTINUE,
                probe.DBG_EXCEPTION_NOT_HANDLED, probe.DBG_EXCEPTION_NOT_HANDLED,
                probe.DBG_CONTINUE,
            ])
            self.assertEqual(api.closed_handles, [10])
            self.assertEqual(api.lookup, (pid, 0x1190))
            self.assertEqual(job.pid, pid)
            self.assertTrue(job.closed)
            self.assertFalse(process.killed)
            self.assertEqual(launch["kwargs"]["creationflags"],
                             probe.CREATE_SUSPENDED | probe.DEBUG_ONLY_THIS_PROCESS | probe.DETACHED_PROCESS)
            self.assertEqual(launch["kwargs"]["stdout"], subprocess.DEVNULL)
            self.assertEqual(launch["kwargs"]["stderr"], subprocess.DEVNULL)
            child_env = launch["kwargs"]["env"]
            self.assertNotIn("GITHUB_TOKEN", child_env)
            self.assertNotIn("CLOUDFLARE_API_TOKEN", child_env)
            self.assertNotIn("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", child_env)
            self.assertNotIn("TAKOSUMI_BROWSER_PROCESS_REAL_SITE", child_env)
            self.assertNotIn("TAKOSUMI_BROWSER_PROCESS_EXTERNAL_LOG", child_env)
            self.assertEqual(child_env["TAKOSUMI_BROWSER_PROCESS_CASE"], str(case))
            self.assertEqual(child_env["TAKOSUMI_BROWSER_PROCESS_APP"], "local")
            self.assertEqual(child_env["TAKOSUMI_BROWSER_PROCESS_SITE"], "https://127.0.0.1:44321")
            self.assertEqual(child_env["TEMP"], str(case / "tmp"))
            self.assertEqual(child_env["TMP"], str(case / "tmp"))

    def test_timeout_is_unknown_and_owned_job_is_closed_for_cleanup(self):
        with tempfile.TemporaryDirectory() as temporary:
            binary, case = self.make_owned_case(Path(temporary))

            class FakeProcess:
                pid = 74
                killed = False

                def wait(self, timeout=None):
                    self.exit_code = 0
                    return 0

                def kill(self):
                    self.killed = True

                def poll(self):
                    return getattr(self, "exit_code", None)

            process = FakeProcess()

            class FakeJob:
                closed = False

                def adopt(self, actual_pid):
                    self.pid = actual_pid

                def terminate(self):
                    api.terminated = True

                def close(self):
                    self.closed = True

            job = FakeJob()

            class TimeoutApi:
                terminated = False

                def wait_for_debug_event(self, timeout_ms):
                    if self.terminated:
                        return probe.DebugEvent(probe.DEBUG_EVENT_EXIT_PROCESS, 74, 1, exit_code=1)
                    return None

                def continue_debug_event(self, event, status):
                    self.continued = (event, status)

                def close_event_handles(self, event):
                    self.closed_event = event

            api = TimeoutApi()
            started = time.monotonic()
            report = probe.run_probe(binary, case, timeout_seconds=1,
                                     site_url="https://127.0.0.1:44321",
                                     popen_factory=lambda *_args, **_kwargs: process,
                                     job_factory=lambda: job, api_factory=lambda: api)
            self.assertFalse(report["diagnosticComplete"])
            self.assertFalse(report["observed"])
            self.assertEqual(report["reason"], "probe_timeout")
            self.assertNotIn("passed", report)
            self.assertTrue(report["jobHandleClosed"])
            self.assertTrue(report["ownedProcessReaped"])
            self.assertTrue(job.closed)
            self.assertTrue(api.terminated)
            self.assertFalse(process.killed)
            self.assertGreaterEqual(time.monotonic() - started, 1)

    def test_unowned_case_and_nonfixture_name_are_rejected_before_launch(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            case = root / "owned"
            case.mkdir()
            (case / ".owned-browser-reference").write_text(case.name, encoding="utf-8")
            wrong_binary = root / "other.exe"
            wrong_binary.write_bytes(b"not launched")
            launches = []
            result = probe.run_probe(wrong_binary, case, site_url="https://127.0.0.1:44321",
                                     popen_factory=lambda *a, **k: launches.append(a))
            self.assertEqual(result["reason"], "fixture_or_platform_unavailable")
            self.assertEqual(launches, [])

            binary = root / "takosumi-mobile.exe"
            binary.write_bytes(b"fixture-placeholder")
            result = probe.run_probe(binary, case, site_url="https://example.invalid/",
                                     popen_factory=lambda *a, **k: launches.append(a))
            self.assertEqual(result["reason"], "fixture_or_platform_unavailable")
            self.assertEqual(launches, [])


if __name__ == "__main__":
    unittest.main()
