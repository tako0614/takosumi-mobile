#!/usr/bin/env python3
"""Portable policy and ownership tests for the Windows native fault probe."""
import json
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path, PureWindowsPath
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import windows_native_fault_probe as probe  # noqa: E402


class EventPolicyTests(unittest.TestCase):
    def test_source_location_keeps_only_checkout_or_public_rust_paths(self):
        repo = PureWindowsPath(r"D:\\a\\private-runner-path\\takosumi-mobile")
        self.assertEqual(probe.source_location(str(repo / "src-tauri/src/lib.rs"), 422, repo),
                         {"file": "src-tauri/src/lib.rs", "line": 422})
        self.assertEqual(probe.source_location(
            r"C:\\Users\\private\\.cargo\\registry\\src\\index.crates.io-1949cf8c6b5b557f\\wry-0.55.1\\src\\lib.rs",
            34, repo), {"file": "wry-0.55.1/src/lib.rs", "line": 34})
        rust_source = "/rustc/" + ("a" * 40) + "/library/std/src/sys/windows/mod.rs"
        self.assertEqual(probe.source_location(rust_source, 128, repo),
                         {"file": rust_source[1:], "line": 128})
        for name in (r"C:\\private\\token.rs", "src/lib.rs", "src-tauri/../private.rs",
                     "C:" + rust_source, "/rustc/private/library/std/src/lib.rs",
                     r"C:\\.cargo\\registry\\src\\private-registry-12345678\\scope-1.2.3\\src\\lib.rs"):
            self.assertIsNone(probe.source_location(name, 34, repo))
        self.assertIsNone(probe.source_location(str(repo / "src-tauri/src/lib.rs"), 0, repo))

    def test_local_symbol_index_needs_both_guid_and_age(self):
        binary, first, second = map(Path, ("test.exe", "test.pdb", "test_fixture.pdb"))
        guid = (301, 22, 19, 9)
        indexes = {str(binary): (guid, 4), str(first): (guid, 3),
                   str(second): ((302, 22, 19, 9), 4)}

        def get_index(path, output, flags):
            self.assertEqual(flags, 0)
            result = output._obj
            self.assertEqual(result.sizeofstruct, 1608)
            entry = indexes.get(path)
            if entry is None:
                return False
            result.guid[:], result.age = entry
            return True

        with patch.object(probe, "_declare", return_value=get_index):
            self.assertEqual(probe.local_symbol_index_status(None, binary, [first, second]), "mismatched")
            indexes[str(second)] = (guid, 4)
            self.assertEqual(probe.local_symbol_index_status(None, binary, [first, second]), "matched")
            del indexes[str(binary)]
            self.assertEqual(probe.local_symbol_index_status(None, binary, [first, second]), "unreadable")

    def test_symbol_line_evidence_distinguishes_no_line_and_filtered_path(self):
        repo = PureWindowsPath(r"D:\a\takosumi-mobile")
        binary = repo / "target/debug/takosumi-mobile.exe"
        frames = [{"module": binary.name, "offset": hex(rva), "fixtureAppImage": True}
                  for rva in (0x100, 0x200, 0x300)]

        def get_line(_process, address, displacement, line):
            if address == 0x11ff:
                return False
            line._obj.Address = address - 3
            displacement._obj.value = 3
            line._obj.LineNumber = 45
            line._obj.FileName = str(repo / "src-tauri/src/lib.rs") if address == 0x10ff else r"C:\private\unknown.rs"
            return True

        native = {
            "GetCurrentProcess": lambda: -1, "SymInitializeW": lambda *_: True,
            "SymCleanup": lambda *_: True, "SymSetOptions": lambda *_: 0,
            "SymLoadModuleExW": lambda *_: 0x1000, "SymGetLineFromAddrW64": get_line,
        }
        evidence = {}
        with (patch.object(probe, "os", SimpleNamespace(name="nt")),
              patch.object(probe.ctypes, "WinDLL", return_value=None, create=True),
              patch.object(PureWindowsPath, "is_symlink", return_value=False, create=True),
              patch.object(PureWindowsPath, "is_file", return_value=True, create=True),
              patch.object(probe, "_declare", side_effect=lambda _dll, name, *_: native[name]),
              patch.object(probe, "local_symbol_index_status", return_value="matched")):
            probe.application_source_lines(frames, binary, repo, evidence)
        self.assertEqual(evidence, {"stage": "completed", "pdbCandidates": 2, "pdbIndex": "matched",
                                    "lookups": 3, "linesFound": 2, "addressMatches": 2, "projectedLines": 1})
        self.assertEqual(frames[0]["source"], {"file": "src-tauri/src/lib.rs", "line": 45})
        self.assertNotIn("source", frames[1])
        self.assertNotIn("source", frames[2])

    def test_caller_frame_limit_counts_unwind_frames_before_module_projection(self):
        case = PureWindowsPath(r"C:\fixture")
        context = probe._Amd64Context(Rip=0x1000, Rsp=0x2000)
        event = probe.DebugEvent(probe.DEBUG_EVENT_EXCEPTION, 73, 1,
                                 probe.EXCEPTION_ACCESS_VIOLATION, False, context.Rip)

        def module_for_address(_pid, address):
            return (None, None) if address == 0xA010 else ("takosumi-mobile.exe", 0xA000)

        api = SimpleNamespace(kernel32=None, module_for_address=module_for_address)
        for available, expected, reached, calls in ((4, 4, False, 6), (18, 16, True, 17)):
            with self.subTest(available=available):
                # A successful initial step can repeat the fault PC/SP.
                raw_frames = iter([(context.Rip, context.Rsp)] + [
                    (0xA000 + 16 * index, 0x2000 + 16 * index) for index in range(1, available + 1)
                ])
                walk_calls = []
                cleaned = []

                def walk(_machine, _process, _thread, frame, *_):
                    current = next(raw_frames, None)
                    walk_calls.append(current)
                    if current is None:
                        return False
                    frame._obj.AddrPC.Offset, frame._obj.AddrStack.Offset = current
                    return True

                native = {
                    "SymInitializeW": lambda *_: True, "SymCleanup": cleaned.append,
                    "SymSetOptions": lambda *_: 0, "StackWalk64": walk,
                    "SymFunctionTableAccess64": None, "SymGetModuleBase64": None,
                }
                with (patch.object(probe.ctypes, "WinDLL", return_value=None, create=True),
                      patch.object(probe, "_declare", side_effect=lambda _dll, name, *_: native[name]),
                      patch.object(probe, "stopped_amd64_frames",
                                   side_effect=lambda *_: probe._unwind(2, 3, context, case))):
                    frames, limit_reached = probe.caller_frame_projection(api, event, case, 0xA000)
                self.assertEqual(len(frames), expected - 1)  # The first caller was unmapped.
                self.assertEqual(frames[-1], {"module": "takosumi-mobile.exe",
                                              "offset": hex(16 * expected), "fixtureAppImage": True})
                self.assertIs(limit_reached, reached)
                self.assertEqual(len(walk_calls), calls)
                self.assertEqual(cleaned, [2])

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
                probe.DebugEvent(probe.DEBUG_EVENT_CREATE_PROCESS, pid, 1, close_handles=(10,), image_base=0xA000),
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
                    if address == 0xC420:
                        return "takosumi-mobile.exe", 0xC000
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
                return iter((0xA420, 0xB425, 0xC420))
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
                                     "callerFrames": [
                                         {"module": "takosumi-mobile.exe", "offset": "0x420", "fixtureAppImage": True},
                                         {"module": "takosumi-mobile.exe", "offset": "0x420"},
                                     ],
                                     "callerFrameLimitReached": False,
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
