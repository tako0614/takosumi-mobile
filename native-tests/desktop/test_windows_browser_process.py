#!/usr/bin/env python3
"""Unit tests of the W3 Windows runner and observer pieces that need no App,
browser or Windows: the WebSocket client, the DevTools event bookkeeping, the
TCP table parser, the fixture site, the receipt check and the case list the
CI job requires. Standard library only; runs on Linux and Windows."""
import base64
import hashlib
import http.client
import http.server
import json
import re
import socket
import struct
import sys
import threading
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import windows_browser_observer as observer  # noqa: E402
import windows_browser_process as runner  # noqa: E402
from windows_smoke import Failure  # noqa: E402


def server_frame(opcode, payload, fin=True):
    """An unmasked server frame."""
    header = bytearray([(0x80 if fin else 0) | opcode])
    if len(payload) < 126:
        header.append(len(payload))
    elif len(payload) < 65536:
        header.append(126)
        header += len(payload).to_bytes(2, "big")
    else:
        header.append(127)
        header += len(payload).to_bytes(8, "big")
    return bytes(header) + payload


def read_client_frame(sock):
    """(opcode, payload) of one masked client frame."""
    def take(count):
        data = b""
        while len(data) < count:
            chunk = sock.recv(count - len(data))
            if not chunk:
                raise ConnectionError("closed")
            data += chunk
        return data
    first, second = take(2)
    assert second & 0x80, "client frames must be masked"
    length = second & 0x7F
    if length == 126:
        length = int.from_bytes(take(2), "big")
    elif length == 127:
        length = int.from_bytes(take(8), "big")
    mask = take(4)
    return first & 0x0F, observer.mask_bytes(take(length), mask)


def accept_handshake(sock):
    request = b""
    while b"\r\n\r\n" not in request:
        request += sock.recv(4096)
    key = re.search(rb"Sec-WebSocket-Key: (\S+)", request).group(1)
    accept = base64.b64encode(hashlib.sha1(key + observer.GUID.encode()).digest())
    sock.sendall(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                 b"Sec-WebSocket-Accept: " + accept + b"\r\n\r\n")
    return request


class WebSocketTests(unittest.TestCase):
    def setUp(self):
        self.client_side, self.server_side = socket.socketpair()
        self.server_side.settimeout(5)
        self.handshake = {}
        thread = threading.Thread(target=lambda: self.handshake.update(request=accept_handshake(self.server_side)))
        thread.start()
        self.websocket = observer.WebSocket("127.0.0.1", 9222, "/devtools/browser/x", connection=self.client_side)
        thread.join(5)

    def tearDown(self):
        self.client_side.close()
        self.server_side.close()

    def test_handshake_names_the_endpoint(self):
        self.assertTrue(self.handshake["request"].startswith(b"GET /devtools/browser/x HTTP/1.1\r\n"))
        self.assertIn(b"Host: 127.0.0.1:9222\r\n", self.handshake["request"])

    def test_messages_of_every_length_and_fragments_with_a_ping(self):
        long = ("x" * 70000).encode()
        self.server_side.sendall(server_frame(0x1, b'{"a":1}') + server_frame(0x1, b"m" * 300) + server_frame(0x1, long)
                                 + server_frame(0x1, b"part-", fin=False) + server_frame(0x9, b"hi")
                                 + server_frame(0x0, b"two"))
        deadline = time.monotonic() + 5
        self.assertEqual(self.websocket.receive(deadline), '{"a":1}')
        self.assertEqual(self.websocket.receive(deadline), "m" * 300)
        self.assertEqual(self.websocket.receive(deadline), "x" * 70000)
        self.assertEqual(self.websocket.receive(deadline), "part-two")
        self.assertEqual(read_client_frame(self.server_side), (0xA, b"hi"))

    def test_a_timed_out_read_loses_no_part_of_a_frame(self):
        frame = server_frame(0x1, b"complete")
        self.server_side.sendall(frame[:5])
        with self.assertRaises(TimeoutError):
            self.websocket.receive(time.monotonic() + 0.2)
        self.server_side.sendall(frame[5:])
        self.assertEqual(self.websocket.receive(time.monotonic() + 5), "complete")

    def test_client_frames_are_masked_text(self):
        self.websocket.send_text("é" * 200)
        self.assertEqual(read_client_frame(self.server_side), (0x1, ("é" * 200).encode()))

    def test_a_close_frame_ends_the_connection(self):
        self.server_side.sendall(server_frame(0x8, b""))
        with self.assertRaises(ConnectionError):
            self.websocket.receive(time.monotonic() + 5)


class FakeSocket:
    """Queued DevTools messages for Cdp; every send is recorded."""

    def __init__(self, messages):
        self.messages = list(messages)
        self.sent = []

    def send_text(self, text):
        self.sent.append(json.loads(text))

    def receive(self, deadline):
        if not self.messages:
            raise TimeoutError
        return json.dumps(self.messages.pop(0))


class CdpTests(unittest.TestCase):
    def test_a_command_returns_its_own_reply_and_keeps_events(self):
        cdp = observer.Cdp(FakeSocket([{"method": "Page.frameNavigated", "params": {}},
                                       {"id": 1, "result": {"ok": True}}]), "work")
        mark = cdp.mark()
        self.assertEqual(cdp.command("Runtime.evaluate", {"expression": "1"}, "S"), {"ok": True})
        self.assertEqual(cdp.websocket.sent, [{"id": 1, "method": "Runtime.evaluate",
                                               "params": {"expression": "1"}, "sessionId": "S"}])
        self.assertEqual([event["method"] for event in cdp.since(mark)], ["Page.frameNavigated"])

    def test_errors_and_timeouts_fail(self):
        cdp = observer.Cdp(FakeSocket([{"id": 1, "error": {"message": "no"}}]), "work")
        with self.assertRaisesRegex(Failure, "failed"):
            cdp.command("Page.enable")
        with self.assertRaisesRegex(Failure, "CDP timeout"):
            cdp.command("Page.enable", timeout=0.1)

    def test_wait_event_looks_only_after_its_mark(self):
        cdp = observer.Cdp(FakeSocket([{"method": "Page.javascriptDialogOpening", "params": {"type": "alert"}}]), "w")
        cdp.drain(0.1)
        mark = cdp.mark()
        cdp.websocket.messages.append({"method": "Page.javascriptDialogOpening", "params": {"type": "beforeunload"}})
        event = cdp.wait_event(lambda e: e["method"] == "Page.javascriptDialogOpening", mark, 1, "none")
        self.assertEqual(event["params"]["type"], "beforeunload")
        with self.assertRaisesRegex(Failure, "nothing more"):
            cdp.wait_event(lambda e: e["method"] == "Page.javascriptDialogClosed", mark, 0.1, "nothing more")

    def test_the_final_quit_accepts_only_the_sites_own_beforeunload(self):
        origin = observer.REAL_SITES["excalidraw"]["origin"]
        self.assertEqual(observer.final_quit_dialog({"type": "beforeunload", "url": "https://excalidraw.com/#x"},
                                                    origin), {"type": "beforeunload", "origin": origin})
        for params in ({"type": "alert", "url": "https://excalidraw.com/"},
                       {"type": "confirm", "url": "https://excalidraw.com/"},
                       {"type": "beforeunload", "url": "https://evil.example/"},
                       {"type": "beforeunload", "url": "http://excalidraw.com/"},
                       {"type": "beforeunload"}):
            with self.assertRaises(Failure, msg=params):
                observer.final_quit_dialog(params, origin)

    def test_the_final_quit_refuses_another_dialog_unanswered(self):
        alert = {"method": "Page.javascriptDialogOpening", "sessionId": "S",
                 "params": {"type": "alert", "url": "https://excalidraw.com/", "message": "x"}}
        cdp = observer.Cdp(FakeSocket([alert]), "personal")
        process = {"opened": {"windowId": 1, "nativePid": 2}, "profile": "P",
                   "guardReceipt": str(HERE / "no-such-receipt.json")}
        page = observer.Page(cdp, "S", process, {"targetId": "T"})
        watch = observer.Observer(HERE / "no-such-case", "https://127.0.0.1:1", 1, "", real_site="excalidraw")
        with self.assertRaisesRegex(Failure, "only a beforeunload"):
            watch.finish_final_quit([page])
        self.assertEqual(watch.proof["finalQuitDialogsAccepted"], [])
        self.assertEqual(watch.proof["finalQuitDialogRefused"]["type"], "alert")
        self.assertEqual(cdp.websocket.sent, [], "nothing may answer the refused dialog")

    def test_marks_stay_absolute_when_old_events_are_dropped(self):
        cdp = observer.Cdp(FakeSocket([{"method": f"E{index}"} for index in range(4200)]), "w")
        cdp.drain(5)
        self.assertEqual(cdp.mark(), 4200)
        self.assertEqual([event["method"] for event in cdp.since(4198)], ["E4198", "E4199"])


class TcpTableTests(unittest.TestCase):
    def row(self, state, address, port, pid):
        return struct.pack("<6I", state, struct.unpack("<I", socket.inet_aton(address))[0],
                           socket.htons(port), 0, 0, pid)

    def test_only_loopback_listeners_on_the_port_count(self):
        rows = [self.row(2, "127.0.0.1", 9222, 10), self.row(2, "0.0.0.0", 9222, 11),
                self.row(5, "127.0.0.1", 9222, 12), self.row(2, "127.0.0.1", 9223, 13)]
        table = struct.pack("<I", len(rows)) + b"".join(rows)
        self.assertEqual(observer.loopback_listeners(table, 9222), [10])

    def test_a_truncated_table_is_refused(self):
        with self.assertRaises(Failure):
            observer.loopback_listeners(struct.pack("<I", 2) + self.row(2, "127.0.0.1", 1, 1), 1)


class SiteTests(unittest.TestCase):
    def setUp(self):
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), runner.Site)
        self.server.requests = []
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()

    def request(self, method, path, body=None, cookie=None):
        connection = http.client.HTTPConnection("127.0.0.1", self.server.server_port, timeout=5)
        headers = {"Cookie": cookie} if cookie else {}
        if body is not None:
            headers["Content-Type"] = "application/x-www-form-urlencoded"
        connection.request(method, path, body=body, headers=headers)
        response = connection.getresponse()
        result = response.status, dict(response.getheaders()), response.read().decode()
        connection.close()
        return result

    def test_sign_in_stand_in_then_home_then_app(self):
        status, _, body = self.request("GET", "/")
        self.assertEqual(status, 200)
        self.assertIn('data-page="login"', body)
        self.assertEqual(self.request("GET", "/app/")[:2][0], 303)
        status, headers, _ = self.request("POST", "/session", "user=work")
        self.assertEqual((status, headers["Location"]), (303, "/"))
        cookie = headers["Set-Cookie"]
        for attribute in ("fixture_session=work", "Secure", "HttpOnly", "SameSite=Lax", "Path=/", "Max-Age=86400"):
            self.assertIn(attribute, cookie)
        session = cookie.split(";", 1)[0]
        self.assertIn('data-page="home" data-user="work"', self.request("GET", "/", cookie=session)[2])
        self.assertIn('data-page="app" data-user="work"', self.request("GET", "/app/", cookie=session)[2])

    def test_the_app_home_links_to_the_real_site_of_a_w4_run(self):
        self.server.app_url = runner.REAL_SITES["excalidraw"]["url"]
        session = self.request("POST", "/session", "user=personal")[1]["Set-Cookie"].split(";", 1)[0]
        self.assertIn('<a data-app="editor" href="https://excalidraw.com/">', self.request("GET", "/", cookie=session)[2])

    def test_only_the_synthetic_users_and_known_paths(self):
        self.assertEqual(self.request("POST", "/session", "user=admin")[0], 400)
        self.assertEqual(self.request("POST", "/other", "user=work")[0], 404)
        self.assertEqual(self.request("GET", "/other")[0], 404)
        self.assertIn('data-page="login"', self.request("GET", "/", cookie="fixture_session=admin")[2])


class RunnerTests(unittest.TestCase):
    def test_case_names_also_fit_the_app_identifier(self):
        for name in ("sumi-w3", "a", "x" * 24):
            self.assertIsNotNone(runner.CASE_NAME.fullmatch(name))
        for name in ("", "-a", "Sumi", "a_b", "a.b", "x" * 25):
            self.assertIsNone(runner.CASE_NAME.fullmatch(name))

    def test_only_a_complete_normal_receipt_passes(self):
        receipt = {"rootExit": 0, "complete": True, "kernelNoChildren": True, "remaining": [], "forcedCleanup": 0,
                   "cleanupErrors": [], "error": None}
        runner.normal(receipt)
        for field, value in (("rootExit", 1), ("complete", False), ("kernelNoChildren", False), ("remaining", [4]),
                             ("forcedCleanup", 1), ("cleanupErrors", ["x"]), ("error", "x"), ("rootExit", True)):
            with self.assertRaises(Failure):
                runner.normal(dict(receipt, **{field: value}))

    def test_the_ci_job_requires_exactly_the_runner_cases(self):
        cases = [name for phase in ("", "restart", "reset") for name in runner.OBSERVED_CASES[phase]]
        cases.insert(len(runner.OBSERVED_CASES[""]), runner.QUIT_CASE)
        cases += list(runner.FINAL_CASES)
        self.assertEqual(len(cases), len(set(cases)))
        workflow = (HERE.parents[1] / ".github/workflows/ci.yml").read_text()
        block = re.search(r"\$w3Cases = @\(\n(.*?)\n\s*\)", workflow, re.S)
        count = re.search(r"\$w3Count -ne (\d+)\)", workflow)
        self.assertIsNotNone(block, "The cef-windows-app job lists its required cases")
        self.assertIsNotNone(count, "The cef-windows-app job requires an exact case count")
        self.assertEqual(re.findall(r"'([a-z-]+)'", block.group(1)), cases)
        self.assertEqual(int(count.group(1)), len(cases))

    def test_the_real_site_workflow_requires_exactly_the_runner_cases(self):
        workflow = (HERE.parents[1] / ".github/workflows/windows-real-sites.yml").read_text()
        for site in sorted(runner.REAL_SITES):
            block = re.search(r"\$%sCases = @\(\n(.*?)\n\s*\)" % site, workflow, re.S)
            count = re.search(r"\$%sCount -ne (\d+)\)" % site, workflow)
            self.assertIsNotNone(block, f"The real-site workflow lists the {site} cases")
            self.assertIsNotNone(count, f"The real-site workflow counts the {site} cases")
            cases = runner.real_site_cases(site)
            self.assertEqual(re.findall(r"'([a-z/-]+)'", block.group(1)), cases)
            self.assertEqual(int(count.group(1)), len(cases))

    def test_the_real_site_workflow_runs_only_when_asked(self):
        workflow = (HERE.parents[1] / ".github/workflows/windows-real-sites.yml").read_text()
        on = re.search(r"^on:\n(.*?)\n(?=\S)", workflow, re.S | re.M).group(1)
        self.assertEqual(re.findall(r"^  (\w+):", on, re.M), ["workflow_dispatch", "pull_request"])
        self.assertRegex(on, r"  pull_request:\n    types: \[labeled\]\n?$")
        self.assertNotIn("pull_request_target", workflow)
        self.assertIn("if: ${{ github.event_name == 'workflow_dispatch' || "
                      "github.event.label.name == 'windows-real-sites' }}", workflow)
        test_ref = ("github.event_name == 'workflow_dispatch' && inputs.ref || "
                    "github.event.pull_request.head.sha")
        self.assertIn(f"TEST_REF: ${{{{ {test_ref} }}}}", workflow)
        # The concurrency group is the job's, after its `if`: a run for any
        # other label skips the job and cannot cancel a pending real run.
        self.assertNotRegex(workflow, r"(?m)^concurrency:")
        self.assertIn("    if: ${{ github.event_name == 'workflow_dispatch' || "
                      "github.event.label.name == 'windows-real-sites' }}\n"
                      "    # On the job, after its `if`: a run for any other label skips the job and\n"
                      "    # so never enters this group (where it could cancel a pending real run).\n"
                      "    concurrency:\n"
                      f"      group: takosumi-mobile-windows-real-sites-${{{{ {test_ref} }}}}\n"
                      "      cancel-in-progress: false\n", workflow)
        self.assertRegex(workflow, r"(?m)^permissions:\n  contents: read$")

    def test_a_dispatch_tests_only_this_repositorys_commits(self):
        workflow = (HERE.parents[1] / ".github/workflows/windows-real-sites.yml").read_text()
        steps = workflow[workflow.index("    steps:\n"):]
        check = steps.index("      - name: This repository's branches (dispatch)\n"
                            "        if: ${{ github.event_name == 'workflow_dispatch' }}\n")
        resolve = steps.index("      - name: Revision under test\n        id: test-ref\n")
        checkout = steps.index("          ref: ${{ steps.test-ref.outputs.sha }}\n          path: takosumi-mobile\n")
        self.assertLess(check, resolve)
        self.assertLess(resolve, checkout)
        self.assertIn("          fetch-depth: 0\n", steps[check:resolve])
        script = steps[resolve:checkout]
        # The input reaches the script only through the environment.
        self.assertNotIn("${{ inputs.ref }}", workflow)
        self.assertIn("$ref = $env:TEST_REF", script)
        self.assertIn("if ($ref -match '^(refs/)?pull/') {", script)
        self.assertIn("rev-parse --verify --quiet", script)
        self.assertIn("branch -r --contains $sha", script)
        self.assertIn('"sha=$env:HEAD_SHA" >> $env:GITHUB_OUTPUT', script)
        self.assertNotIn("ref: ${{ env.TEST_REF }}", workflow)

    def test_the_real_site_workflow_checks_out_the_ci_sibling_revisions(self):
        root = HERE.parents[1] / ".github/workflows"
        refs = [dict(re.findall(r"^  (MOBILE_KIT_SOURCE_REF|TAKOSUMI_SOURCE_REF): (\S+)$", (root / name).read_text(), re.M))
                for name in ("ci.yml", "windows-real-sites.yml")]
        self.assertEqual(len(refs[0]), 2)
        self.assertEqual(refs[0], refs[1])

    def test_real_sites_are_anonymous_https_roots_with_a_readiness_signal(self):
        for site, entry in observer.REAL_SITES.items():
            self.assertTrue(entry["url"].startswith("https://") and entry["url"].endswith("/"), site)
            self.assertEqual(entry["url"].rstrip("/"), entry["origin"])
            self.assertTrue(entry["ready"])
            self.assertEqual("landing" in entry, "enter" in entry, site)

    def test_the_real_site_bounds_cover_the_observers_worst_case(self):
        tests = HERE.parents[1] / "src-tauri/tests/desktop"
        inspect = re.search(r"if shell::real_site\(\) \{\s*return Duration::from_secs\((\d+)\);",
                            (tests / "browser_process.rs").read_text())
        watchdog = re.search(r"from_secs\(if real_site\(\) \{ (\d+) \} else \{ (\d+) \}\)",
                             (tests / "browser_process_windows.rs").read_text())
        self.assertIsNotNone(inspect, "browser_process.rs names the real-site first-checkpoint bound")
        self.assertIsNotNone(watchdog, "browser_process_windows.rs names the real-site watchdog")
        inspect = int(inspect.group(1))
        real_watchdog, watchdog = (int(value) for value in watchdog.groups())
        worst = max(observer.real_site_first_checkpoint(site) for site in observer.REAL_SITES)
        self.assertEqual(worst, 4 * observer.LOAD_WAIT + 4 * observer.REAL_SITE_WAIT
                         + 4 * observer.FOREGROUND_WAIT + observer.OCCLUSION_WATCH + 2 * observer.SETTLE_WAIT)
        # A margin for the DevTools commands between the bounded waits.
        self.assertGreaterEqual(inspect, max(worst + 60, worst * 1.15))
        # The observer gets past that checkpoint and still ends before the
        # App's watchdog, which keeps the ordinary 90 s for the rest.
        self.assertGreater(runner.REAL_SITE_OBSERVER_WAIT, inspect)
        self.assertGreater(real_watchdog, runner.REAL_SITE_OBSERVER_WAIT)
        self.assertGreaterEqual(real_watchdog - inspect, watchdog)
        self.assertGreater(watchdog, runner.OBSERVER_WAIT)

    def test_real_site_readiness_needs_the_rendered_editor(self):
        # Excalidraw's crash screen also carries `.excalidraw`; Photopea's
        # static landing shows even if the editor's code never loads.
        excalidraw = observer.REAL_SITES["excalidraw"]["ready"]
        self.assertIn("document.querySelector('.excalidraw canvas')", excalidraw)
        self.assertIn("!document.querySelector('.ErrorSplash')", excalidraw)
        photopea = observer.REAL_SITES["photopea"]
        self.assertNotIn("Start using Photopea", photopea["ready"])
        self.assertIn("Open From Computer", photopea["ready"])
        self.assertIn("Start using Photopea", photopea["landing"])
        probe = (HERE / "browser-process.mjs").read_text()
        self.assertIn("document.querySelector('.excalidraw canvas')!==null && "
                      "!document.querySelector('.ErrorSplash')", probe)
        self.assertNotIn("Boolean(document.querySelector('.excalidraw'))", probe)

    def test_session_files_must_be_plain_and_bounded(self):
        # Read-only: this test file stands in for a session file.
        file = Path(__file__)
        self.assertEqual(runner.read_session(file), file.read_bytes())
        with self.assertRaises(Failure):
            runner.read_session(file, 4)
        with self.assertRaises(Failure):
            runner.read_session(HERE)


if __name__ == "__main__":
    unittest.main()
