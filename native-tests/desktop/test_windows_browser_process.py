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
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import windows_browser_observer as observer  # noqa: E402
import windows_browser_process as runner  # noqa: E402
import windows_browser_fault as fault  # noqa: E402
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


class ReplyingSocket(FakeSocket):
    """FakeSocket that answers each command from `replies` (method: the
    reply, or a list of the reply and the events that follow it)."""

    def __init__(self, messages, replies):
        super().__init__(messages)
        self.replies = replies

    def send_text(self, text):
        super().send_text(text)
        request = self.sent[-1]
        reply = self.replies[request["method"]]
        reply, *after = reply if isinstance(reply, list) else [reply]
        self.messages.append(dict(reply, id=request["id"]))
        self.messages.extend(after)


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
                 "params": {"type": "alert", "url": "https://excalidraw.com/#json=secret", "message": "x"}}
        cdp = observer.Cdp(FakeSocket([alert]), "personal")
        process = {"opened": {"windowId": 1, "nativePid": 2}, "profile": "P",
                   "guardReceipt": str(HERE / "no-such-receipt.json")}
        page = observer.Page(cdp, "S", process, {"targetId": "T"})
        watch = observer.Observer(HERE / "no-such-case", "https://127.0.0.1:1", 1, "", real_site="excalidraw")
        with self.assertRaisesRegex(Failure, "only a beforeunload"):
            watch.finish_final_quit([page])
        self.assertEqual(watch.proof["finalQuitDialogsAccepted"], [])
        self.assertEqual(watch.proof["finalQuitDialogRefused"],
                         {"profile": "P", "type": "alert", "origin": "https://excalidraw.com"})
        self.assertNotIn("secret", json.dumps(watch.proof), "only the origin of a real site's URL is kept")
        self.assertEqual(cdp.websocket.sent, [], "nothing may answer the refused dialog")

    def test_the_final_quit_requires_normal_cleanup_before_reporting_success(self):
        normal = {"rootExit": 0, "complete": True, "kernelNoChildren": True,
                  "remaining": [], "forcedCleanup": 0, "cleanupErrors": [], "error": None}
        # Run 43179155497: Personal's browser exited 0, but its guard had to
        # kill ten descendants. Any JSON receipt used to end this loop and
        # mark the observer passed, hiding the reason for the App's refusal
        # behind the outer guard's subsequent 45-second timeout.
        for changed in ({"forcedCleanup": 10}, {"rootExit": 2},
                        {"remaining": [17]}, {"complete": False},
                        {"kernelNoChildren": False}, {"cleanupErrors": ["failed"]},
                        {"error": "failed"}, {}):
            with self.subTest(changed=changed), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "guard.json"
                receipt = dict(normal, **changed)
                path.write_text(json.dumps(receipt))
                cdp = observer.Cdp(FakeSocket([]), "personal")
                page = observer.Page(cdp, "S", {
                    "opened": {"windowId": 1, "nativePid": 2}, "profile": "Personal",
                    "guardReceipt": str(path)}, {"targetId": "T"})
                watch = observer.Observer(Path(directory), "https://127.0.0.1:1", 1, "",
                                          real_site="photopea")
                if changed:
                    with self.assertRaisesRegex(Failure, "Final quit.*Personal.*abnormal receipt"):
                        watch.finish_final_quit([page])
                else:
                    watch.finish_final_quit([page])
                self.assertEqual(watch.proof["finalQuitReceipts"],
                                 [{"profile": "Personal", "receipt": receipt}])
                self.assertEqual(cdp.websocket.sent, [], "an ended profile needs no CDP commands")

    def test_photopea_is_entered_from_its_landing_and_excalidraw_directly(self):
        class FakePage:
            def __init__(self, site, profile, editor_first=False):
                self.site, self.entered, self.clicks = observer.REAL_SITES[site], editor_first, []
                self.process = {"profile": profile, "opened": {"windowId": 1, "nativePid": 2}}
                self.native, self.target = self.process["opened"], {"targetId": profile}

            def evaluate(self, expression, gesture=False):
                site = self.site
                if "a[data-app=editor]')?.href" in expression:
                    return site["url"]
                if expression == site.get("enter"):
                    self.clicks.append(gesture)
                    self.entered = True
                    return True
                if expression.startswith("({origin:location.origin"):
                    return {"origin": site["origin"], "secure": True, "tauri": "undefined", "ipc": "undefined",
                            "wry": "undefined", "webview2": "undefined"}
                if "landing" in site and expression.endswith(f"({site['ready']} || {site['landing']})"):
                    return True  # the landing is up
                if expression == site.get("landing"):
                    return not self.entered
                if expression.endswith(site["ready"]):
                    return self.entered or "landing" not in site
                return True  # the app link's click

            def pages(self):
                return [self.target["targetId"]]

        checked = []
        sandbox = lambda watch, process, stage="attach": checked.append((process["profile"], stage))
        for site, entry, clicks in (("photopea", "landing", [True]), ("excalidraw", "direct", [])):
            watch = observer.Observer(HERE / "no-such-case", "https://127.0.0.1:1", 1, "", real_site=site)
            pages = [FakePage(site, "W"), FakePage(site, "P")]
            checked.clear()
            with mock.patch.object(observer.Observer, "sandbox", sandbox):
                watch.open_real_site(pages)
            for page in pages:
                self.assertEqual(page.clicks, clicks, site)
                self.assertEqual(page.site_context["entry"], entry, site)
                self.assertEqual(page.site_context["origin"], observer.REAL_SITES[site]["origin"])
            # W7: each profile's sandbox is checked again on the real site.
            self.assertEqual(checked, [("W", "real-site"), ("P", "real-site")], site)
        # A fresh Photopea profile whose editor is up without its landing fails.
        watch = observer.Observer(HERE / "no-such-case", "https://127.0.0.1:1", 1, "", real_site="photopea")
        pages = [FakePage("photopea", "W", editor_first=True), FakePage("photopea", "P")]
        with self.assertRaisesRegex(Failure, "must show photopea's landing before its editor"):
            watch.open_real_site(pages)
        self.assertEqual(pages[0].clicks, [])

    def test_dialog_evidence_keeps_events_and_origins_without_text(self):
        events = [
            {"method": "Page.javascriptDialogOpening", "sessionId": "S", "_receivedAt": 10.5,
             "params": {"type": "beforeunload", "message": "Leave?", "url": "https://www.photopea.com/p?q=1",
                        "hasBrowserHandler": True, "defaultPrompt": "secret"}},
            {"method": "Page.frameAttached", "sessionId": "S", "_receivedAt": 10.6,
             "params": {"frameId": "F2", "parentFrameId": "T"}},
            {"method": "Page.frameNavigated", "sessionId": "S", "_receivedAt": 10.7,
             "params": {"frame": {"id": "F2", "parentId": "T", "url": "https://ads.example/x?id=secret"}}},
            {"method": "Page.frameRequestedNavigation", "sessionId": "S",
             "params": {"frameId": "F2", "url": "https://ads.example/y#secret", "reason": "other"}},
            {"method": "Page.javascriptDialogOpening", "sessionId": "OTHER", "params": {"type": "alert"}},
            {"method": "Page.loadEventFired", "sessionId": "S", "params": {}},
            {"method": "Page.javascriptDialogClosed", "sessionId": "S", "_receivedAt": 11.0,
             "params": {"result": True, "userInput": "secret"}},
            {"method": "Page.javascriptDialogClosed", "sessionId": "S", "params": {"result": False, "userInput": ""}},
        ]
        evidence = observer.dialog_evidence(events, "S", 10.0)
        self.assertEqual(evidence["dialogs"], [
            {"event": "javascriptDialogOpening", "at": 0.5, "type": "beforeunload",
             "origin": "https://www.photopea.com", "hasBrowserHandler": True},
            {"event": "javascriptDialogClosed", "at": 1.0, "result": True, "userInput": "present"},
            {"event": "javascriptDialogClosed", "result": False, "userInput": "empty"}])
        self.assertEqual(evidence["frames"], [
            {"event": "frameAttached", "at": 0.6, "frameId": "F2", "parentId": "T"},
            {"event": "frameNavigated", "at": 0.7, "frameId": "F2", "parentId": "T", "origin": "https://ads.example"},
            {"event": "frameRequestedNavigation", "frameId": "F2", "origin": "https://ads.example"}])
        self.assertEqual(evidence["frameEvents"], 3)
        self.assertNotIn("secret", json.dumps(evidence))
        self.assertNotIn("Leave?", json.dumps(evidence))
        many = [{"method": "Page.frameStartedLoading", "sessionId": "S", "params": {"frameId": f"F{index}"}}
                for index in range(observer.FRAME_EVIDENCE_LIMIT + 50)]
        evidence = observer.dialog_evidence(many, "S", None)
        self.assertEqual(len(evidence["frames"]), observer.FRAME_EVIDENCE_LIMIT)
        self.assertEqual(evidence["frames"][-1]["frameId"], f"F{observer.FRAME_EVIDENCE_LIMIT + 49}")
        self.assertEqual(evidence["frameEvents"], observer.FRAME_EVIDENCE_LIMIT + 50)

    def test_a_failed_cancel_records_both_pages_and_answers_nothing(self):
        def page(label, session, events):
            cdp = observer.Cdp(ReplyingSocket([], {
                "Page.handleJavaScriptDialog": {"error": {"code": -32602, "message": "No dialog is showing"}},
                "Target.getTargets": {"result": {"targetInfos": [{"type": "page", "targetId": label}]}}}), label)
            mark = cdp.mark()
            cdp.websocket.messages.extend(events)
            cdp.drain(0.1)
            process = {"opened": {"windowId": 1, "nativePid": 2}, "profile": label,
                       "guardReceipt": str(HERE / f"no-such-receipt-{label}.json")}
            return observer.Page(cdp, session, process, {"targetId": label}), mark

        opening = {"method": "Page.javascriptDialogOpening", "sessionId": "P",
                   "params": {"type": "beforeunload", "message": "x", "url": "https://www.photopea.com/"}}
        closed = {"method": "Page.javascriptDialogClosed", "sessionId": "P", "params": {"result": True,
                                                                                       "userInput": ""}}
        personal, personal_mark = page("personal", "P", [opening, closed])
        work, work_mark = page("work", "W", [])
        # The local-fixture run: only the observer's own cancel counts.
        watch = observer.Observer(HERE / "no-such-case", "https://127.0.0.1:1", 1, "")
        opened = personal.cdp.since(personal_mark)[0]
        with self.assertRaisesRegex(Failure, "No dialog is showing"):
            watch.cancel_dialog(personal, personal_mark, "cancel not observed", opened, [(work, work_mark)])
        [evidence] = watch.proof["dialogFailures"]
        self.assertEqual(evidence["what"], "cancel not observed")
        self.assertIn("No dialog is showing", evidence["error"])
        self.assertGreaterEqual(evidence["cancelAfterOpening"], 0)
        first, second = evidence["pages"]
        self.assertEqual([entry["event"] for entry in first["dialogs"]],
                         ["javascriptDialogOpening", "javascriptDialogClosed"])
        self.assertEqual(first["dialogs"][1]["result"], True)
        self.assertEqual((first["profile"], first["mainFrame"], first["targets"], first["guardEnded"]),
                         ("personal", "personal", ["personal"], False))
        self.assertEqual((second["profile"], second["dialogs"], second["guardEnded"]), ("work", [], False))
        # Only the one cancel attempt: no accept, no retry.
        self.assertEqual([request["method"] for request in personal.cdp.websocket.sent],
                         ["Page.handleJavaScriptDialog", "Target.getTargets"])
        self.assertEqual(personal.cdp.websocket.sent[0]["params"], {"accept": False})

    def cancel(self, real_site, events, reply, guard_ended=False):
        """cancel_dialog on a page whose events since its mark are `events`
        (the first one is the dialog's opening) and whose cancel gets `reply`."""
        cdp = observer.Cdp(ReplyingSocket([], {"Page.handleJavaScriptDialog": reply,
                                               "Target.getTargets": {"result": {"targetInfos": []}}}), "P")
        mark = cdp.mark()
        cdp.websocket.messages.extend(events)
        cdp.drain(0.1)
        process = {"opened": {"windowId": 1, "nativePid": 2}, "profile": "P",
                   "guardReceipt": str(HERE / "no-such-receipt.json")}
        page = observer.Page(cdp, "S", process, {"targetId": "T"})
        watch = observer.Observer(HERE / "no-such-case", "https://127.0.0.1:1", 1, "", real_site=real_site)
        watch.guard_ended = lambda receipt: guard_ended
        try:
            return watch, watch.cancel_dialog(page, mark, "cancel", cdp.since(mark)[0]), cdp
        finally:
            self.sent = [request["method"] for request in cdp.websocket.sent]

    OPENING = {"method": "Page.javascriptDialogOpening", "sessionId": "S",
               "params": {"type": "beforeunload", "message": "", "url": "https://www.photopea.com/"}}
    NO_DIALOG = {"error": {"code": -32602, "message": "No dialog is showing"}}

    @staticmethod
    def closed(result):
        return {"method": "Page.javascriptDialogClosed", "sessionId": "S", "params": {"result": result,
                                                                                     "userInput": ""}}

    def test_a_cancel_closed_by_the_observer(self):
        for site in (None, "photopea"):
            watch, closed_by, _ = self.cancel(site, [self.OPENING], [{"result": {}}, self.closed(False)])
            self.assertEqual(closed_by, "observer", site)
            self.assertNotIn("dialogsClosedByBrowser", watch.proof)
            self.assertEqual(self.sent, ["Page.handleJavaScriptDialog"])

    def test_a_real_site_may_cancel_its_dialog_before_the_observer(self):
        watch, closed_by, _ = self.cancel("photopea", [self.OPENING, self.closed(False)], self.NO_DIALOG)
        self.assertEqual(closed_by, "browser")
        [record] = watch.proof["dialogsClosedByBrowser"]
        self.assertEqual(record["profile"], "P")
        self.assertIn("No dialog is showing", record["error"])
        self.assertGreaterEqual(record["closedAfterOpening"], 0)
        self.assertNotIn("dialogFailures", watch.proof)
        self.assertEqual(self.sent, ["Page.handleJavaScriptDialog"])
        # The close may also arrive just after DevTools' answer.
        watch, closed_by, _ = self.cancel("photopea", [self.OPENING], [self.NO_DIALOG, self.closed(False)])
        self.assertEqual(closed_by, "browser")

    def test_only_a_real_sites_own_cancel_of_a_running_profile_counts(self):
        refused = (
            # The local-fixture run requires the observer's cancel.
            (None, [self.OPENING, self.closed(False)], False, "No dialog is showing"),
            # Accepted, not cancelled.
            ("photopea", [self.OPENING, self.closed(True)], False, "result True, not cancelled"),
            # The profile left although its dialog was cancelled.
            ("photopea", [self.OPENING, self.closed(False)], True, "guard ended"),
            # Another dialog opened first.
            ("photopea", [self.OPENING, self.OPENING, self.closed(False)], False, "Another dialog"),
        )
        for site, events, guard_ended, message in refused:
            with self.assertRaisesRegex(Failure, message, msg=message):
                self.cancel(site, events, self.NO_DIALOG, guard_ended)
        with mock.patch.object(observer, "SETTLE_WAIT", 0.3):
            with self.assertRaisesRegex(Failure, "close was not observed"):
                self.cancel("photopea", [self.OPENING], self.NO_DIALOG)

    def test_marks_stay_absolute_when_old_events_are_dropped(self):
        cdp = observer.Cdp(FakeSocket([{"method": f"E{index}"} for index in range(4200)]), "w")
        cdp.drain(5)
        self.assertEqual(cdp.mark(), 4200)
        self.assertEqual([event["method"] for event in cdp.since(4198)], ["E4198", "E4199"])

    def test_a_starved_read_rides_the_sent_requests_queue_position(self):
        calls = []

        class FakePage:
            def evaluate(self, expression, gesture=False, timeout=5.0):
                calls.append(timeout)
                return {"ok": True}

        watch = observer.Observer(HERE / "no-such-case", "https://127.0.0.1:1", 1, "", real_site="excalidraw")
        self.assertEqual(watch.read_evaluate(FakePage(), "1"), {"ok": True})
        # The whole starved bound waits on the request already sent — it is
        # not re-queued behind a still-deep renderer task queue.
        self.assertEqual(len(calls), 1)
        self.assertGreater(calls[0], 5.0)

    def test_a_timed_out_starved_read_is_recorded_before_raising(self):
        calls = []

        class FakePage:
            def evaluate(self, expression, gesture=False, timeout=5.0):
                calls.append(timeout)
                raise Failure("CDP timeout Runtime.evaluate (P)")

        watch = observer.Observer(HERE / "no-such-case", "https://127.0.0.1:1", 1, "", real_site="excalidraw")
        with mock.patch.object(observer, "STARVED_READ_WAIT", 0.001):
            with self.assertRaisesRegex(Failure, "CDP timeout"):
                watch.read_evaluate(FakePage(), "1")
        retries = watch.proof["starvedEvaluateRetries"]
        self.assertEqual(retries[0]["attempt"], 1)
        self.assertIn("CDP timeout", retries[0]["error"])

    def test_a_starved_read_reraises_non_timeout_failures_unrecorded(self):
        class FakePage:
            def evaluate(self, expression, gesture=False, timeout=5.0):
                raise Failure("CDP Runtime.evaluate failed (P): boom")

        watch = observer.Observer(HERE / "no-such-case", "https://127.0.0.1:1", 1, "", real_site="excalidraw")
        with self.assertRaisesRegex(Failure, "boom"):
            watch.read_evaluate(FakePage(), "1")
        self.assertNotIn("starvedEvaluateRetries", watch.proof)

    def contended_page(self, sends, events, evaluate=None, base=0):
        class FakeCdp:
            def __init__(self):
                self.base = base

            # Faithful to Cdp.since only while base == 0; tests needing a
            # nonzero base refuse before reaching this scan.
            def since(self, mark):
                return events[mark:]

        class FakePage:
            def __init__(self):
                self.cdp = FakeCdp()
                self.session = "S1"
                self.target = {"targetId": "T1"}
                self.process = {"profile": "P"}
                self.calls = []

            def send(self, method, params=None, timeout=5.0):
                self.calls.append(method)
                result = sends[method]
                if isinstance(result, Exception):
                    raise result
                return result

            def evaluate(self, expression, gesture=False, timeout=5.0):
                self.calls.append("Runtime.evaluate")
                return evaluate or {"document": "0:n", "storage": "0:n",
                                    "cookie": "0:n", "origin": "https://site.test"}

            def pages(self):
                return [self.target["targetId"]]

        sends.setdefault(
            "Page.getFrameTree",
            {"frameTree": {"frame": {"id": "F", "securityOrigin": "https://site.test"}}})
        return FakePage()

    def test_a_contended_sample_reads_state_off_the_renderer_queue(self):
        key = "sumi_app_process_no-such-case"
        page = self.contended_page(
            {"DOMStorage.enable": {},
             "DOMStorage.getDOMStorageItems": {"entries": [[key, "0:n"], ["other", "x"]]},
             "Network.getCookies": {"cookies": [{"name": key, "value": "0:n"}]}}, [])
        watch = observer.Observer(HERE / "no-such-case", "https://127.0.0.1:1", 1, "", real_site="excalidraw")
        self.assertEqual(watch.contended_sample(page, 0),
                         {"storage": "0:n", "cookie": "0:n", "origin": "https://site.test"})
        self.assertNotIn("Runtime.evaluate", page.calls)

    def test_a_contended_sample_refuses_a_main_frame_navigation(self):
        page = self.contended_page(
            {"DOMStorage.enable": {},
             "DOMStorage.getDOMStorageItems": {"entries": []},
             "Network.getCookies": {"cookies": []}},
            [{"method": "Page.frameNavigated", "sessionId": "S1",
              "params": {"frame": {"id": "F2", "parentId": "F"}}},
             {"method": "Page.frameNavigated", "sessionId": "S1",
              "params": {"frame": {"id": "F"}}}])
        watch = observer.Observer(HERE / "no-such-case", "https://127.0.0.1:1", 1, "", real_site="excalidraw")
        with self.assertRaisesRegex(Failure, "replaced"):
            watch.contended_sample(page, 0)

    def test_a_contended_sample_refuses_a_main_frame_load(self):
        page = self.contended_page(
            {"DOMStorage.enable": {},
             "DOMStorage.getDOMStorageItems": {"entries": []},
             "Network.getCookies": {"cookies": []}},
            [{"method": "Page.frameStartedLoading", "sessionId": "S1",
              "params": {"frameId": "F"}}])
        watch = observer.Observer(HERE / "no-such-case", "https://127.0.0.1:1", 1, "", real_site="excalidraw")
        with self.assertRaisesRegex(Failure, "began replacing"):
            watch.contended_sample(page, 0)

    def test_a_contended_sample_refuses_a_crashed_renderer(self):
        page = self.contended_page(
            {"DOMStorage.enable": {},
             "DOMStorage.getDOMStorageItems": {"entries": []},
             "Network.getCookies": {"cookies": []}},
            [{"method": "Inspector.targetCrashed", "sessionId": "S1", "params": {}}])
        watch = observer.Observer(HERE / "no-such-case", "https://127.0.0.1:1", 1, "", real_site="excalidraw")
        with self.assertRaisesRegex(Failure, "crashed"):
            watch.contended_sample(page, 0)

    def test_a_contended_sample_refuses_dropped_event_coverage(self):
        page = self.contended_page(
            {"DOMStorage.enable": {},
             "DOMStorage.getDOMStorageItems": {"entries": []},
             "Network.getCookies": {"cookies": []}}, [], base=5)
        watch = observer.Observer(HERE / "no-such-case", "https://127.0.0.1:1", 1, "", real_site="excalidraw")
        with self.assertRaisesRegex(Failure, "were dropped"):
            watch.contended_sample(page, 0)
        self.assertTrue(page.calls, "the sends precede the coverage check")

    def test_a_contended_sample_falls_back_when_the_storage_domains_are_missing(self):
        missing = observer.CdpError("CDP DOMStorage.enable failed (P): method missing",
                                  {"message": "method missing"})
        page = self.contended_page(
            {"DOMStorage.enable": missing,
             "DOMStorage.getDOMStorageItems": {"entries": []},
             "Network.getCookies": {"cookies": []}}, [])
        watch = observer.Observer(HERE / "no-such-case", "https://127.0.0.1:1", 1, "", real_site="excalidraw")
        self.assertEqual(watch.contended_sample(page, 0)["storage"], "0:n")
        self.assertIn("Runtime.evaluate", page.calls)
        self.assertEqual(len(watch.proof["browserSampleFallbacks"]), 1)

    def test_a_contended_sample_rides_out_a_starved_browser_read(self):
        # Run 43090171027: the storm that still answered every control-pipe
        # command starved getFrameTree's flat SETTLE_WAIT. The send claims
        # its position and waits out one shared bound instead of failing on
        # the first 10 s.
        key = "sumi_app_process_no-such-case"
        starved = {"n": 0}
        def flaky(method):
            starved["n"] += 1
            if starved["n"] == 1:
                return Failure("CDP timeout Page.getFrameTree (P)")
            return {"frameTree": {"frame": {"id": "F", "securityOrigin": "https://site.test"}}}
        class Sends(dict):
            def setdefault(self, key, default=None):
                # Keep the key absent so __missing__ keeps producing the
                # flaky results; contended_page would otherwise pin a
                # static default tree.
                return default if key == "Page.getFrameTree" else super().setdefault(key, default)
            def __missing__(self, method):
                return flaky(method) if method == "Page.getFrameTree" else {}
        sends = Sends({"DOMStorage.enable": {},
                       "DOMStorage.getDOMStorageItems": {"entries": [[key, "0:n"]]},
                       "Network.getCookies": {"cookies": [{"name": key, "value": "0:n"}]}})
        page = self.contended_page(sends, [])
        watch = observer.Observer(HERE / "no-such-case", "https://127.0.0.1:1", 1, "", real_site="excalidraw")
        self.assertEqual(watch.contended_sample(page, 0)["storage"], "0:n")
        self.assertEqual([call for call in page.calls if call == "Page.getFrameTree"],
                         ["Page.getFrameTree", "Page.getFrameTree"])
        retries = watch.proof["starvedBrowserReadRetries"]
        self.assertEqual(retries[0]["method"], "Page.getFrameTree")
        self.assertIn("CDP timeout", retries[0]["error"])

    def test_a_contended_sample_names_a_starved_browser_read_after_the_bound(self):
        page = self.contended_page(
            {"Page.getFrameTree": Failure("CDP timeout Page.getFrameTree (P)"),
             "DOMStorage.enable": {},
             "DOMStorage.getDOMStorageItems": {"entries": []},
             "Network.getCookies": {"cookies": []}}, [])
        watch = observer.Observer(HERE / "no-such-case", "https://127.0.0.1:1", 1, "", real_site="excalidraw")
        with mock.patch.object(observer, "STARVED_READ_WAIT", 0.001):
            with self.assertRaisesRegex(Failure, "CDP timeout Page.getFrameTree"):
                watch.contended_sample(page, 0)
        self.assertEqual(watch.proof["starvedBrowserReadRetries"][0]["method"],
                         "Page.getFrameTree")

    def contended_page_with_events(self, sends, events):
        page = self.contended_page(sends, events)
        page.cdp.events = events
        return page

    def test_a_starved_read_extends_only_while_the_storm_stays_live(self):
        # Run 43173821786: DOMStorage.enable starved the full base bound
        # while renderer churn kept the event stream alive. A stream still
        # receiving events names a starved channel, so the bound extends in
        # quiet-window steps and the late answer still lands.
        key = "sumi_app_process_no-such-case"
        attempts = {"n": 0}

        def flaky(method):
            attempts["n"] += 1
            if attempts["n"] <= 3:
                return Failure("CDP timeout DOMStorage.enable (P)")
            return {}

        class Sends(dict):
            def __missing__(self, method):
                return flaky(method) if method == "DOMStorage.enable" else {}

        sends = Sends({"DOMStorage.getDOMStorageItems": {"entries": [[key, "0:n"]]},
                       "Network.getCookies": {"cookies": [{"name": key, "value": "0:n"}]}})
        events = [{"method": "Target.targetCreated", observer.RECEIVED_AT: time.monotonic()}]
        page = self.contended_page_with_events(sends, events)
        watch = observer.Observer(HERE / "no-such-case", "https://127.0.0.1:1", 1, "", real_site="excalidraw")
        with mock.patch.multiple(observer, STARVED_READ_WAIT=0.01,
                                 STARVED_QUIET_WAIT=0.05, STARVED_MAX_WAIT=5.0):
            self.assertEqual(watch.contended_sample(page, 0)["storage"], "0:n")
        self.assertEqual(attempts["n"], 4)
        retries = watch.proof["starvedBrowserReadRetries"]
        self.assertEqual(len(retries), 3)
        self.assertTrue(all(retry["method"] == "DOMStorage.enable" for retry in retries))
        self.assertTrue(all(retry["channelQuietSeconds"] == 0.0 for retry in retries))

    def test_a_starved_read_still_names_the_method_past_the_hard_cap(self):
        # Storm liveness extends only up to STARVED_MAX_WAIT: a channel that
        # never quietens and never answers raises on the method's own name
        # rather than waiting forever. send keeps the event stream fresh, so
        # the extension path is what must give up — a dead-stream run would
        # end on the soft deadline and never exercise the cap at all.
        events = [{"method": "Target.targetCreated", observer.RECEIVED_AT: time.monotonic()}]

        class Cdp:
            base = 0

            def since(self, mark):
                return events[mark:]

        class Page:
            def __init__(self):
                self.attempts = 0
                self.cdp = Cdp()
                self.cdp.events = [{"method": "Target.targetCreated",
                                    observer.RECEIVED_AT: time.monotonic()}]
                self.session = "S1"
                self.process = {"profile": "P"}

            def send(self, method, params=None, timeout=5.0):
                self.attempts += 1
                # A broken bound without the hard cap would retry forever;
                # fail the test on a distinct name instead of hanging.
                if self.attempts > 100000:
                    raise Failure("starved read ignored the hard cap")
                self.cdp.events.append(
                    {"method": "Target.targetCreated",
                     observer.RECEIVED_AT: time.monotonic()})
                raise Failure(f"CDP timeout {method} (P)")

        watch = observer.Observer(HERE / "no-such-case", "https://127.0.0.1:1", 1, "", real_site="excalidraw")
        with mock.patch.multiple(observer, STARVED_READ_WAIT=0.01,
                                 STARVED_QUIET_WAIT=0.01, STARVED_MAX_WAIT=0.05):
            with self.assertRaisesRegex(Failure, "CDP timeout Page.getFrameTree"):
                watch.contended_sample(Page(), 0)
        retries = watch.proof["starvedBrowserReadRetries"]
        self.assertTrue(retries)
        self.assertTrue(all(retry["method"] == "Page.getFrameTree" for retry in retries))


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


class SandboxEvidenceTests(unittest.TestCase):
    """windows_smoke.py --expect-sandbox (W7): the pure parts."""

    def test_command_lines_split_like_the_c_runtime(self):
        import windows_smoke as smoke
        self.assertEqual(smoke.windows_arguments('"C:\\a b\\x.exe" --type=renderer "--lang=en US" c\\d'),
                         ["C:\\a b\\x.exe", "--type=renderer", "--lang=en US", "c\\d"])
        self.assertEqual(smoke.windows_arguments('x.exe "e\\"f" g\\\\"h i" "j""k"'),
                         ["x.exe", 'e"f', "g\\h i", 'j"k'])
        self.assertEqual(smoke.process_type(smoke.windows_arguments("x.exe --type=renderer --x")), "renderer")
        self.assertIsNone(smoke.process_type(smoke.windows_arguments("x.exe -- --type=renderer")))
        self.assertIsNone(smoke.process_type(smoke.windows_arguments("--type=gpu-process x")))
        self.assertTrue(smoke.has_no_sandbox(smoke.windows_arguments("x.exe --type=renderer --no-sandbox")))
        self.assertFalse(smoke.has_no_sandbox(smoke.windows_arguments("x.exe -- --no-sandbox")))

    def test_only_restricted_renderers_below_the_browser_show_the_sandbox(self):
        import windows_smoke as smoke
        browser = {"pid": 1, "integrity": 0x2000, "argumentsRead": True, "noSandbox": False}
        untrusted = {"pid": 2, "integrity": 0x0000, "noSandbox": False}
        low = {"pid": 3, "integrity": 0x1000, "noSandbox": False}
        self.assertEqual(smoke.sandbox_problems(browser, [untrusted, low]), [])
        cases = (
            (browser, [], "no renderer"),
            (browser, [{"pid": 4, "integrity": 0x2000, "noSandbox": False}], "medium integrity"),
            (browser, [{"pid": 5, "integrity": 0x0000, "noSandbox": True}], "--no-sandbox"),
            (browser, [{"pid": 6, "integrity": None, "noSandbox": False, "error": "denied"}], "unknown"),
            ({"pid": 1, "integrity": 0x2000, "noSandbox": True}, [untrusted], "the browser runs with --no-sandbox"),
            ({"pid": 1, "integrity": 0x1000, "noSandbox": False}, [low], "not below the browser's"),
            ({"pid": 1, "integrity": 0x2000, "argumentsRead": False, "noSandbox": False, "error": "denied"}, [low],
             "the browser's command line was not read (denied)"),
        )
        for owner, renderers, problem in cases:
            problems = smoke.sandbox_problems(owner, renderers)
            self.assertTrue(any(problem in text for text in problems), (problem, problems))


    def test_a_live_process_whose_command_line_was_not_read_is_a_problem(self):
        import windows_smoke as smoke
        browser = {"pid": 1, "integrity": 0x2000, "argumentsRead": True, "noSandbox": False}
        renderer = {"pid": 2, "integrity": 0x0000, "noSandbox": False}
        exited = {"pid": 3, "error": "OpenProcess: Windows error 87", "exited": True}
        live = {"pid": 4, "error": "NtQueryInformationProcess: 0xC0000022", "exited": False}
        self.assertEqual(smoke.sandbox_problems(browser, [renderer], [exited]), [])
        self.assertEqual(smoke.sandbox_problems(browser, [renderer], [exited, live]),
                         ["process 4: command line not read (NtQueryInformationProcess: 0xC0000022)"])
        # The browser's own unread command line is reported once, as the browser's.
        unread_browser = dict(browser, argumentsRead=False, error="denied")
        problems = smoke.sandbox_problems(unread_browser, [renderer], [{"pid": 1, "error": "denied"}])
        self.assertEqual(problems, ["the browser's command line was not read (denied)"])
        self.assertEqual(smoke.ERROR_INVALID_PARAMETER, 87)

    def test_an_unread_process_that_ends_within_a_second_has_exited(self):
        """One being terminated can lose its readable command line while it
        still reads as running: it is waited for (UNREAD_WAIT) before it
        counts as a problem; the browser's own is never waited for."""
        import windows_smoke as smoke
        asked = []

        def ended(pid):
            asked.append(pid)
            return pid == 4

        unread = [{"pid": 1, "error": "x", "exited": False}, {"pid": 3, "error": "y", "exited": True},
                  {"pid": 4, "error": "z", "exited": False}, {"pid": 5, "error": "w", "exited": False}]
        settled = smoke.settle_unread(unread, 1, ended)
        self.assertEqual(asked, [4, 5])
        self.assertEqual(settled[2], {"pid": 4, "error": "z", "exited": True, "endedAfterWait": True})
        self.assertEqual([process["exited"] for process in settled], [False, True, True, False])
        browser = {"pid": 1, "integrity": 0x2000, "argumentsRead": True, "noSandbox": False}
        renderer = {"pid": 2, "integrity": 0x0000, "noSandbox": False}
        self.assertEqual(smoke.sandbox_problems(browser, [renderer], settled),
                         ["process 5: command line not read (w)"])
        self.assertEqual(smoke.UNREAD_WAIT, 1.0)

        class Kernel32:
            def __init__(self, handle, signaled):
                self.handle, self.signaled, self.closed, self.waits = handle, signaled, 0, []

            def OpenProcess(self, access, inherit, pid):
                self.access = access
                return self.handle

            def WaitForSingleObject(self, handle, milliseconds):
                self.waits.append(milliseconds)
                return 0 if self.signaled else 0x102  # WAIT_TIMEOUT

            def CloseHandle(self, handle):
                self.closed += 1

        running = Kernel32(7, False)
        self.assertFalse(smoke.ended_within(running, 9, 1.0))
        self.assertEqual((running.access, running.waits, running.closed), (0x00100000, [1000], 1))
        self.assertTrue(smoke.ended_within(Kernel32(7, True), 9, 1.0))
        with mock.patch.object(smoke.ctypes, "get_last_error", lambda: 87, create=True):
            self.assertTrue(smoke.ended_within(Kernel32(None, False), 9, 1.0))
        with mock.patch.object(smoke.ctypes, "get_last_error", lambda: 5, create=True):
            self.assertFalse(smoke.ended_within(Kernel32(None, False), 9, 1.0))

    def test_a_browsers_process_tree_is_its_descendants_by_parent(self):
        import windows_smoke as smoke
        pairs = [(0, 0), (4, 0), (10, 4), (20, 10), (21, 10), (30, 20), (40, 4), (50, 99), (60, 60), (61, 62),
                 (62, 61)]
        self.assertEqual(smoke.descendant_pids(pairs, 10), [20, 21, 30])
        self.assertEqual(smoke.descendant_pids(pairs, 30), [])
        self.assertEqual(smoke.descendant_pids(pairs, 60), [])
        self.assertEqual(smoke.descendant_pids(pairs, 61), [62])

        class Tree(smoke.ProcessTree):
            def __init__(self, root, times=None):
                self.root, self.times = root, times or {}

            def pairs(self):
                return pairs

            def created(self, pid):
                return self.times.get(pid)

        self.assertEqual(Tree(10).process_ids(), [10, 20, 21, 30])
        # PID reuse: a process created before its parent is not its child
        # (21 is older than 10; 30 is older than 20, so not reached either).
        times = {10: 100, 20: 150, 21: 90, 30: 120}
        self.assertEqual(smoke.descendant_pids(pairs, 10, times.get), [20])
        self.assertEqual(Tree(10, times).process_ids(), [10, 20])
        # An unknown creation time keeps the edge.
        self.assertEqual(smoke.descendant_pids(pairs, 10, {10: 100, 21: 90}.get), [20, 30])
        # PROCESSENTRY32W: 568 bytes on x64 Windows (two-byte WCHAR).
        ctypes = smoke.ctypes
        if ctypes.sizeof(ctypes.c_void_p) == 8:
            self.assertEqual(ctypes.sizeof(smoke.ProcessEntry), (44 + 260 * ctypes.sizeof(ctypes.c_wchar) + 7) // 8 * 8)
        self.assertEqual(smoke.X64_SIZES[smoke.ProcessEntry], 568)

    def test_the_observer_requires_each_profiles_browser_to_show_the_sandbox(self):
        found = {"browser": {"pid": 7, "integrityName": "high"}, "renderers": [], "problems": ["no renderer"]}
        calls = []

        def evidence(source, pid, deadline):
            calls.append((source.root, pid))
            return found

        class Win:
            kernel32 = None

        process = {"profile": "p-work", "opened": {"nativePid": 7}}
        with mock.patch.object(observer, "sandbox_evidence", evidence), \
                mock.patch.object(observer.ProcessTree, "__init__",
                                  lambda tree, kernel32, root: setattr(tree, "root", root)):
            required = observer.Observer(Path("case"), "https://127.0.0.1:1", 1, "", win32=Win())
            with self.assertRaises(Failure) as raised:
                required.sandbox(process)
            self.assertIn("The browser of p-work does not show Chromium's sandbox", str(raised.exception))
            self.assertEqual(required.proof["sandbox"], [dict(found, profile="p-work", stage="attach", required=True)])
            recorded = observer.Observer(Path("case"), "https://127.0.0.1:1", 1, "", win32=Win(), expect_sandbox=False)
            self.assertEqual(recorded.sandbox(process, "real-site"), found)
            self.assertEqual(recorded.proof["sandbox"],
                             [dict(found, profile="p-work", stage="real-site", required=False)])
        self.assertEqual(calls, [(7, 7), (7, 7)])
        self.assertIn("sandbox", runner.EVIDENCE_KEYS)

    def test_the_runner_rechecks_every_opened_profiles_sandbox_record(self):
        opened = {"processes": [{"profile": "work"}, {"profile": "personal"}]}
        good = lambda profile, stage="attach": {"profile": profile, "stage": stage, "browser": {"integrityName": "high"},
                                                "renderers": [{"integrityName": "untrusted"}], "problems": []}
        observation = {"sandbox": [good("work"), good("personal")]}
        self.assertEqual(runner.sandbox_summary(observation, opened, True), [
            {"profile": "work", "stage": "attach", "browser": "high", "renderers": ["untrusted"], "problems": []},
            {"profile": "personal", "stage": "attach", "browser": "high", "renderers": ["untrusted"],
             "problems": []},
        ])
        # A real-site run needs both stages for every profile.
        stages = ("attach", "real-site")
        with self.assertRaisesRegex(Failure, "No real-site sandbox evidence for the browser of work"):
            runner.sandbox_summary(observation, opened, True, stages)
        both = {"sandbox": observation["sandbox"] + [good("work", "real-site"), good("personal", "real-site")]}
        self.assertEqual(len(runner.sandbox_summary(both, opened, True, stages)), 4)
        with self.assertRaisesRegex(Failure, "No attach sandbox evidence for the browser of personal"):
            runner.sandbox_summary({"sandbox": [good("work")]}, opened, True)
        bad = dict(good("personal"), problems=["no renderer process was found"])
        with self.assertRaisesRegex(Failure, "personal does not show Chromium's sandbox"):
            runner.sandbox_summary({"sandbox": [good("work"), bad]}, opened, True)
        # The unsandboxed candidate: recorded, not required.
        self.assertEqual(len(runner.sandbox_summary({"sandbox": [good("work"), bad]}, opened, False)), 2)
        self.assertEqual(runner.sandbox_summary({}, opened, False), [])


class MacSmokeTests(unittest.TestCase):
    """windows_smoke.py's macOS path (M1): the process table, the seatbelt
    sandbox evidence and the descendants left after the exit. Platform
    neutral: `ps`, `sandbox_check` and the process signals are stand-ins,
    and a real call to either of the first two fails the test."""

    def setUp(self):
        import windows_smoke as smoke
        for name in ("ps_rows", "seatbelt_sandboxed"):
            patcher = mock.patch.object(smoke, name, mock.Mock(side_effect=AssertionError(f"real {name} called")))
            patcher.start()
            self.addCleanup(patcher.stop)

    PS = ("    1     0 /sbin/launchd\n"
          "  500     1 /Users/runner/work/_temp/m1/takosumi-cef-browser-process.app/Contents/MacOS/"
          "takosumi-cef-browser-process --no-first-run --use-mock-keychain --lang=en-US\n"
          "  501   500 /Users/runner/work/_temp/m1/takosumi-cef-browser-process.app/Contents/Frameworks/"
          "takosumi-cef-browser-process Helper (GPU).app/Contents/MacOS/takosumi-cef-browser-process Helper (GPU)"
          " --type=gpu-process --lang=en-US\n"
          "  502   500 /Users/x/takosumi-cef-browser-process Helper (Renderer).app/Contents/MacOS/"
          "takosumi-cef-browser-process Helper (Renderer) --type=renderer --seatbelt-client=40\n"
          "  503   502 /bin/sleep 1\n"
          "  600     1 /usr/bin/other --type=renderer\n"
          "garbage line\n")

    def test_the_process_table_and_the_browsers_tree(self):
        import windows_smoke as smoke
        rows = smoke.parse_ps(self.PS)
        self.assertEqual([row[:2] for row in rows], [(1, 0), (500, 1), (501, 500), (502, 500), (503, 502), (600, 1)])
        self.assertTrue(rows[3][2].endswith("--type=renderer --seatbelt-client=40"))
        tree = smoke.posix_tree(500, rows)
        self.assertEqual([pid for pid, _ in tree], [500, 501, 502, 503])
        self.assertEqual(smoke.process_type(tree[2][1].split()), "renderer")
        self.assertIsNone(smoke.process_type(tree[0][1].split()))

    def test_only_sandboxed_renderers_without_no_sandbox_show_the_seatbelt(self):
        import windows_smoke as smoke
        rows = smoke.parse_ps(self.PS)
        tree = lambda root: smoke.posix_tree(root, rows)
        found = smoke.seatbelt_evidence(500, time.monotonic(), tree, lambda pid: pid == 502)
        self.assertEqual(found["renderers"], [{"pid": 502, "noSandbox": False, "sandboxed": True}])
        self.assertEqual(found["browser"], {"pid": 500, "noSandbox": False, "sandboxed": False})
        self.assertEqual(found["types"], ["browser", "gpu-process", "renderer"])
        self.assertEqual(found["problems"], [])
        unsandboxed = smoke.seatbelt_evidence(500, time.monotonic(), tree, lambda pid: False)
        self.assertEqual(unsandboxed["problems"], ["renderer 502 is not in the seatbelt sandbox (sandbox_check: False)"])
        unknown = smoke.seatbelt_evidence(500, time.monotonic(), tree, lambda pid: None)
        self.assertIn("(sandbox_check: None)", unknown["problems"][0])
        opted_out = smoke.parse_ps(self.PS.replace("--type=renderer --seatbelt-client=40", "--type=renderer --no-sandbox")
                                   .replace("--use-mock-keychain", "--no-sandbox"))
        problems = smoke.seatbelt_problems(
            {"pid": 500, "noSandbox": True}, [{"pid": 502, "noSandbox": True, "sandboxed": True}])
        self.assertEqual(problems, ["the browser runs with --no-sandbox", "renderer 502 runs with --no-sandbox"])
        none = smoke.seatbelt_evidence(501, time.monotonic(), lambda root: smoke.posix_tree(root, opted_out),
                                       lambda pid: True)
        self.assertEqual(none["problems"], ["no renderer process was found"])

    def test_a_recorded_descendant_still_alive_is_a_leftover(self):
        """Platform neutral: `ps` and os.killpg are stand-ins, so no real
        process is looked at or signalled."""
        import windows_smoke as smoke

        def no_group(pid, signal_number):
            raise ProcessLookupError(pid)

        session = smoke.Browser.__new__(smoke.Browser)
        session.job = None
        session.process = type("Process", (), {"pid": 500})()
        helper = "/b.app/Contents/Frameworks/b Helper (Renderer).app/Contents/MacOS/b Helper (Renderer) --type=renderer"
        session.descendants = [(501, "/b.app/x --type=gpu-process"), (502, helper)]
        rows = [(502, 1, helper)]  # an orphan: its parent is launchd now
        with mock.patch.object(smoke, "MACOS", True), mock.patch.object(smoke, "LEFTOVER_WAIT", 0), \
                mock.patch.object(smoke.os, "killpg", no_group, create=True), \
                mock.patch.object(smoke, "ps_rows", lambda: rows):
            self.assertEqual(session.leftovers(), ({"groupAlive": False, "descendantsAlive": [502]}, False))
            rows[:] = [(502, 1, "/usr/bin/reused --other")]  # the id now names another process
            self.assertEqual(session.leftovers(), ({"groupAlive": False, "descendantsAlive": []}, True))

    def test_only_a_descendant_still_the_same_process_is_killed(self):
        import windows_smoke as smoke
        recorded = [(501, "/b x --type=gpu-process"), (502, "/b y --type=renderer"), (503, "/b z --type=utility")]
        rows = [(501, 500, "/b x --type=gpu-process"), (502, 1, "/b y --type=renderer"),
                (503, 77, "/usr/bin/somebody-else")]
        self.assertEqual(smoke.surviving(recorded, rows), [501, 502])
        self.assertEqual(smoke.surviving(recorded, []), [])
        killed = []

        def kill(pid, signal_number):
            killed.append((pid, signal_number))

        session = smoke.Browser.__new__(smoke.Browser)
        session.job = None
        session.descendants = recorded
        session.process = mock.Mock(pid=500)
        with mock.patch.object(smoke.os, "killpg", lambda pid, number: None, create=True), \
                mock.patch.object(smoke.os, "kill", kill), \
                mock.patch.object(smoke.signal, "SIGKILL", 9, create=True), \
                mock.patch.object(smoke, "ps_rows", lambda: rows):
            session.end_all()
        self.assertEqual(killed, [(501, 9), (502, 9)])
        session.process.wait.assert_called_once()


def main_source():
    return (HERE.parents[1] / "native-tests/cef-candidate/src/process_main.rs").read_text(encoding="utf-8")


class FakeProcessApi:
    """Handle objects keep their generation even when a PID is reused."""
    def __init__(self):
        self.images = {role: rf"C:\fixture\{role}.exe" for role in ("app", "guard", "browser")}
        self.pids = {"app": 10, "guard": 20, "browser": 30}
        self.objects = {pid: {"pid": pid, "created": pid * 100, "image": fault.image_key(self.images[role]),
                              "live": True, "exit": None} for role, pid in self.pids.items()}
        self.parent_ids = {30: 20, 20: 10}
        self.opened, self.closed, self.signals = [], [], []
        self.fail_open = None

    def open(self, pid, terminate=False):
        if pid == self.fail_open:
            raise Failure("synthetic process access refusal")
        handle = self.objects[pid]
        self.opened.append((pid, terminate))
        return handle

    def identity(self, handle):
        return {key: handle[key] for key in ("pid", "created", "image")}

    def live(self, handle):
        return handle["live"]

    def parents(self):
        return dict(self.parent_ids)

    def terminate(self, handle):
        self.signals.append(handle)
        handle.update(live=False, exit=fault.FAULT_EXIT)

    def wait(self, handle, milliseconds):
        if handle["live"]:
            raise Failure("synthetic process is still live")

    def exit_code(self, handle):
        return handle["exit"]

    def close(self, handle):
        self.closed.append(handle)


class WindowsFaultTests(unittest.TestCase):
    def recovery_layout(self):
        root = Path(self.enterContext(tempfile.TemporaryDirectory()))
        app_data, runtime = root / "local-app-data" / "fixture-case", root / "case" / "r"
        profile = "11111111-1111-4111-8111-111111111111"
        directory = app_data / "browser-process-experiment" / profile
        cache = directory / "cef-profiles" / profile
        cache.mkdir(parents=True)
        receipt = runtime / "owned-guard" / "guard.json"
        receipt.parent.mkdir(parents=True)
        receipt.write_text("")
        process = {"profile": profile, "directory": str(directory), "guardReceipt": str(receipt),
                   "opened": {"nativeCache": str(cache)}}
        return process, app_data, runtime

    def test_persistent_profile_is_in_app_data_while_guard_state_is_in_runtime(self):
        process, app_data, runtime = self.recovery_layout()
        paths = fault.recovery_paths(process, app_data, runtime)
        # On Windows, the runner's Temp path can use RUNNER~1 while strict
        # resolution returns runneradmin. Compare the same canonical paths.
        self.assertEqual(paths["cache"], Path(process["opened"]["nativeCache"]).resolve(strict=True))
        self.assertTrue(paths["cache"].is_relative_to(app_data.resolve(strict=True)))
        self.assertFalse(paths["cache"].is_relative_to(runtime.resolve(strict=True)))
        self.assertTrue(paths["receipt"].resolve(strict=True).is_relative_to(runtime.resolve(strict=True)))

    def test_runtime_or_another_app_is_not_a_persistent_profile_owner(self):
        process, app_data, runtime = self.recovery_layout()
        for wrong in (runtime, app_data.parent):
            with self.assertRaises(Failure):
                fault.recovery_paths(process, wrong, runtime)

    def test_foreign_guard_receipt_and_mismatched_native_cache_are_refused(self):
        process, app_data, runtime = self.recovery_layout()
        foreign = app_data / "foreign.json"
        foreign.write_text("{}")
        wrong = dict(process, guardReceipt=str(foreign))
        with self.assertRaises(Failure):
            fault.recovery_paths(wrong, app_data, runtime)
        wrong = dict(process, opened={"nativeCache": str(app_data)})
        with self.assertRaises(Failure):
            fault.recovery_paths(wrong, app_data, runtime)

    def test_only_the_held_browser_can_receive_the_explicit_fixture_fault(self):
        api = FakeProcessApi()
        with fault.OwnedBrowser(api, api.pids, api.images, allow_fault=True) as owned:
            proof = owned.fault()
            self.assertEqual(proof["exit"], 73)
            self.assertEqual(api.opened, [(10, False), (20, False), (30, True)])
            self.assertEqual(api.signals, [owned.handles["browser"]])
            self.assertTrue(api.objects[10]["live"])
            self.assertTrue(api.objects[20]["live"])
            with self.assertRaises(Failure):
                owned.fault()
            self.assertEqual(len(api.signals), 1)
        self.assertEqual(len(api.closed), 3)

    def test_observation_only_lineages_never_acquire_termination_access(self):
        api = FakeProcessApi()
        with fault.OwnedBrowser(api, api.pids, api.images) as owned:
            with self.assertRaises(Failure):
                owned.fault()
        self.assertEqual(api.signals, [])
        self.assertFalse(any(terminate for _, terminate in api.opened))

    def test_wrong_parent_image_birth_or_dead_owner_refuses_before_fault(self):
        for defect in ("parent", "image", "birth", "dead", "missing-parent"):
            with self.subTest(defect=defect):
                api = FakeProcessApi()
                with fault.OwnedBrowser(api, api.pids, api.images, allow_fault=True) as owned:
                    if defect == "parent":
                        api.parent_ids[30] = 999
                    elif defect == "image":
                        api.objects[20]["image"] = fault.image_key(r"C:\foreign\guard.exe")
                    elif defect == "birth":
                        api.objects[30]["created"] = 1
                    elif defect == "dead":
                        api.objects[10]["live"] = False
                    else:
                        del api.parent_ids[20]
                    with self.assertRaises(Failure):
                        owned.fault()
                    self.assertEqual(api.signals, [])

    def test_pid_reuse_cannot_redirect_a_held_browser_fault(self):
        api = FakeProcessApi()
        with fault.OwnedBrowser(api, api.pids, api.images, allow_fault=True) as owned:
            original = api.objects[30]
            original["live"] = False
            api.objects[30] = dict(original, created=9999, live=True)
            with self.assertRaises(Failure):
                owned.fault()
            self.assertEqual(api.signals, [])
            self.assertTrue(api.objects[30]["live"])

    def test_partial_capture_refusal_closes_existing_handles(self):
        api = FakeProcessApi()
        api.fail_open = 30
        with self.assertRaises(Failure):
            fault.OwnedBrowser(api, api.pids, api.images, allow_fault=True)
        self.assertEqual(api.opened, [(10, False), (20, False)])
        self.assertEqual(len(api.closed), 2)
        self.assertEqual(api.signals, [])

    def test_owner_ending_during_ancestry_inspection_refuses_the_fault(self):
        api = FakeProcessApi()
        with fault.OwnedBrowser(api, api.pids, api.images, allow_fault=True) as owned:
            def owner_ends():
                api.objects[10]["live"] = False
                return dict(api.parent_ids)
            api.parents = owner_ends
            with self.assertRaises(Failure):
                owned.fault()
            self.assertEqual(api.signals, [])


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

    def test_processes_left_in_the_apps_job_are_described(self):
        """Diagnostics: each leftover's pid, image, parent pid and creation
        time (no command line), in the receipt and the failure message."""
        import calendar
        import windows_smoke as smoke
        self.assertIsNone(smoke.filetime_iso(0))
        self.assertIsNone(smoke.filetime_iso(None))
        self.assertEqual(smoke.filetime_iso(smoke.FILETIME_UNIX_EPOCH), "1970-01-01T00:00:00.000Z")
        unix = calendar.timegm((2026, 11, 7, 11, 49, 40, 0, 0, 0))
        self.assertEqual(smoke.filetime_iso(smoke.FILETIME_UNIX_EPOCH + unix * 10**7 + 1_239_999),
                         "2026-11-07T11:49:40.123Z")
        records = [{"pid": 4120, "image": "msedgewebview2.exe", "parent": 3900, "created": "2026-11-07T11:49:40.123Z"},
                   {"pid": 4188, "image": None, "parent": None, "created": None}]
        self.assertEqual(smoke.describe_processes(records),
                         "4120 msedgewebview2.exe (parent 3900, created 2026-11-07T11:49:40.123Z); "
                         "4188 ? (parent ?, created ?)")
        self.assertEqual(smoke.describe_processes([]), "")

        class Tree:
            def __init__(self, kernel32, root):
                pass

            def entries(self):
                return [(4120, 3900, "msedgewebview2.exe"), (3900, 1, "takosumi-mobile.exe")]

            def created(self, pid):
                return {4120: smoke.FILETIME_UNIX_EPOCH}.get(pid)

        with mock.patch.object(smoke, "ProcessTree", Tree):
            self.assertEqual(smoke.process_records(None, [4120, 4188]), [
                {"pid": 4120, "image": "msedgewebview2.exe", "parent": 3900, "created": "1970-01-01T00:00:00.000Z"},
                {"pid": 4188, "image": None, "parent": None, "created": None}])

        class Job:
            kernel32 = None

            def processes(self):
                return 7, 2

        with mock.patch.object(runner, "LEFTOVER_WAIT", 0), mock.patch.object(runner, "job_pids", lambda job: [4120, 4188]), \
                mock.patch.object(runner, "process_records", lambda kernel32, pids: records):
            receipt = runner.session_receipt(Job(), 0)
        self.assertEqual((receipt["remaining"], receipt["remainingProcesses"], receipt["observed"]),
                         ([4120, 4188], records, 7))
        with self.assertRaisesRegex(Failure, "The App's job must have no remaining child: 4120 msedgewebview2.exe "
                                             r"\(parent 3900, created 2026-11-07T11:49:40.123Z\); 4188 \?"):
            runner.normal(receipt, "The App's job")
        with mock.patch.object(runner, "process_records", mock.Mock(side_effect=OSError("denied"))):
            self.assertEqual(runner.leftover_records(Job(), [1]), {"remainingProcessesError": "denied"})
        # A guard receipt has no descriptions: the ids as before.
        with self.assertRaisesRegex(Failure, r"The guard must have no remaining child: \[4\]"):
            runner.normal({"rootExit": 0, "complete": True, "kernelNoChildren": False, "remaining": [4]})

    def test_the_ci_job_requires_exactly_the_runner_cases(self):
        cases = [name for phase in ("", "restart", "reset") for name in runner.OBSERVED_CASES[phase]]
        cases.insert(len(runner.OBSERVED_CASES[""]), runner.QUIT_CASE)
        cases += list(runner.FINAL_CASES)
        self.assertEqual(len(cases), len(set(cases)))
        workflow = (HERE.parents[1] / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        block = re.search(r"\$w3Cases = @\(\n(.*?)\n\s*\)", workflow, re.S)
        count = re.search(r"\$w3Count -ne (\d+)\)", workflow)
        self.assertIsNotNone(block, "The cef-windows-app job lists its required cases")
        self.assertIsNotNone(count, "The cef-windows-app job requires an exact case count")
        self.assertEqual(re.findall(r"'([a-z-]+)'", block.group(1)), cases)
        self.assertEqual(int(count.group(1)), len(cases))

    def test_the_package_job_runs_the_packaged_cases_and_uploads_no_binary(self):
        workflow = (HERE.parents[1] / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        job = workflow[workflow.index("\n  cef-windows-package:\n"):workflow.index("\n  cef-macos:\n")]
        cases = [name for phase in ("", "restart", "reset") for name in runner.OBSERVED_CASES[phase]]
        cases.insert(len(runner.OBSERVED_CASES[""]), runner.QUIT_CASE)
        cases += list(runner.FINAL_CASES)
        block = re.search(r"\$w6Cases = @\(\n(.*?)\n\s*\)", job, re.S)
        self.assertIsNotNone(block, "The package job lists its required cases")
        self.assertEqual(re.findall(r"'([a-z-]+)'", block.group(1)), cases)
        self.assertIn(f"if ($w6Count -ne {len(cases)})", job)
        self.assertIn("windows_browser_process.py --packaged --binary $env:W6_FIXTURE_APP ", job)
        for step in ("windows_package.py install", "windows_package.py smoke", "windows_package.py uninstall"):
            self.assertIn(step, job)
        # The installed product browser alone, and each installer's own
        # uninstall key and shortcut (the fixture has its own productName).
        self.assertIn('windows_smoke.py --browser "$env:RUNNER_TEMP\\w6-app\\cef\\takosumi-cef-browser-process.exe" '
                      '--work $work --expect-sandbox ', job)
        # W7: both stages are the sandboxed browser (the DLL CEF's bootstrap
        # loads); the product stage no longer asks for the unsandboxed one.
        product_stage = job[job.index("- name: Build and stage the product browser and guard"):]
        product_stage = product_stage[:product_stage.index("\n      - name:")]
        self.assertIn("cargo build --locked --features windows-sandbox --lib --bin takosumi-browser-guard\n",
                      product_stage)
        self.assertIn('node scripts/stage-windows-cef.mjs --cef $cef[0].FullName --bin "$env:CARGO_TARGET_DIR\\debug"\n',
                      product_stage)
        self.assertNotIn("unsandboxed-candidate", job.replace(".unsandboxedCandidate -ne $false", ""))
        # Each uninstall names the productName its installer was built with.
        for report, config in (("w6-fixture-uninstall.json", "src-tauri/tauri.windows-cef-fixture.conf.json"),
                               ("w6-uninstall.json", "src-tauri/tauri.windows-cef.conf.json")):
            line = next(line for line in job.splitlines() if f'--report "$env:RUNNER_TEMP\\{report}"' in line)
            self.assertTrue(line.rstrip().endswith("--product $product"), line)
            before = job[:job.index(line)]
            self.assertTrue(before.rstrip().endswith("(Get-Content -Raw src-tauri/tauri.conf.json | ConvertFrom-Json)"
                                                     ".productName }"), before[-300:])
            self.assertIn(f"$product = (Get-Content -Raw {config} | ConvertFrom-Json).productName",
                          before[before.rindex("- name:"):])
        self.assertNotIn("--product TakosumiFixture", job)
        # The fixture stage keeps the product guard, and the two stages must
        # record the same one (the packaged run then runs the product guard).
        fixture = job[job.index("- name: Build and stage the fixture browser"):]
        fixture = fixture[:fixture.index("\n      - name:")]
        self.assertIn("--features fixture-inspection,windows-sandbox --lib\n", fixture)
        self.assertNotIn("--bin takosumi-browser-guard", fixture)
        self.assertIn("if ($guards.Count -ne 2 -or $guards[0] -ne $guards[1]) { throw", fixture)
        overlay = json.loads((HERE.parents[1] / "src-tauri/tauri.windows-cef-fixture.conf.json").read_text(
            encoding="utf-8"))
        self.assertEqual((overlay["productName"], overlay["identifier"]),
                         ("TakosumiFixture", "com.takosumi.fixture.package"))
        # Each installer step takes exactly its own overlay's installer, by
        # productName (the base config's when the overlay has none).
        for step, config in (("Build the fixture installer", "src-tauri/tauri.windows-cef-fixture.conf.json"),
                             ("Build the product installer", "src-tauri/tauri.windows-cef.conf.json")):
            body = job[job.index(f"- name: {step} (NSIS, unsigned)"):]
            body = body[:body.index("\n      - name:")]
            self.assertIn(f"--config {config}", body)
            self.assertIn(f"$product = (Get-Content -Raw {config} | ConvertFrom-Json).productName", body)
            self.assertIn("$product = (Get-Content -Raw src-tauri/tauri.conf.json | ConvertFrom-Json).productName", body)
            self.assertIn('Where-Object { $_.Name -like "$($product)_*_x64-setup.exe" }', body)
            self.assertIn("if ($setup.Count -ne 1)", body)
            self.assertNotIn("-Filter '*-setup.exe'", body)
        product = json.loads((HERE.parents[1] / "src-tauri/tauri.conf.json").read_text(encoding="utf-8"))
        # The product's installer name pattern cannot match the fixture's.
        self.assertFalse(re.fullmatch(re.escape(product["productName"]) + r"_.*_x64-setup\.exe",
                                      "TakosumiFixture_0.1.0_x64-setup.exe"))
        # The upload takes one gathered directory; the gathering copies only
        # named reports through Copy-Evidence and standalone browser text
        # through the SDK-free, tested allowlist copier.
        upload = job[job.index("name: cef-windows-package-evidence"):]
        self.assertIn("path: ${{ runner.temp }}/w6-evidence\n          if-no-files-found", upload)
        gather = job[job.index("- name: Gather the W6 evidence"):job.index("- name: Keep the W6 evidence")]
        self.assertIn("if: ${{ always() }}", gather)
        self.assertIn("$never = @('.exe', '.dll', '.pdb', '.lib', '.msi', '.sys', '.node')", gather)
        self.assertIn("$text = @('.json', '.log', '.txt', '.html', '.md')", gather)
        self.assertEqual(gather.count("Copy-Item"), 1, "report copies go through Copy-Evidence")
        self.assertIn("python -B takosumi-mobile/native-tests/desktop/windows_package_evidence.py "
                      "--source $tree --destination (Join-Path $out $name)", gather)
        self.assertIn("'cef-profiles'", gather)
        self.assertIn("native-tests/desktop/windows_package_evidence.py", runner.WINDOWS_SOURCES)
        self.assertIn("if ($never -contains $extension -or ($allowed -and $allowed -notcontains $extension) "
                      "-or (Test-Binary $file.FullName)) {", gather)
        # MZ, ELF and ZIP by content; a failed copy is logged, not fatal.
        for magic in ("$head[0] -eq 0x4D -and $head[1] -eq 0x5A",
                      "$head[0] -eq 0x7F -and $head[1] -eq 0x45 -and $head[2] -eq 0x4C -and $head[3] -eq 0x46",
                      "$head[0] -eq 0x50 -and $head[1] -eq 0x4B -and $head[2] -eq 0x03 -and $head[3] -eq 0x04"):
            self.assertIn(magic, gather)
        self.assertIn("} catch {\n              $skipped.Add(", gather)
        self.assertIn('foreach ($path in $skipped) { "skipped: $path" }', gather)
        # Every source passes the text allowlist.
        self.assertEqual(gather.count("Copy-Evidence $file") + gather.count("Copy-Evidence (Get-Item"),
                         gather.count(" $text\n") + gather.count(" $text }"))
        sources = ["foreach ($pattern in @('w6-*.json', 'w6-*.log'))",
                   "'product-stage.json' = 'takosumi-mobile/src-tauri/target/windows-cef/stage.json'",
                   "'fixture-stage.json' = 'takosumi-mobile/src-tauri/target/windows-cef-fixture/stage.json'",
                   "$trees = @{ 'case' = $env:W6_CASE; 'browser-smoke' = (Join-Path $env:RUNNER_TEMP 'w6-browser-smoke') }",
                   "Get-ChildItem -LiteralPath $tree -Recurse -File",
                   "foreach ($name in @('browser.log', 'cef-native.log'))"]
        for source in sources:
            self.assertIn(source, gather)
        self.assertEqual(gather.count("Get-ChildItem -LiteralPath"), 4, "no other source is gathered")
        for forbidden in ("setup", "bundle", "target-w3", "*.exe", "*.dll"):
            self.assertNotIn(forbidden, gather, f"the evidence must not include {forbidden}")

    def test_the_w1_job_runs_the_staged_sandboxed_browser(self):
        """W7: cef-windows builds the browser DLL, stages it beside CEF's
        bootstrap and runs every smoke, the guard and the adapter tests with
        the staged files; each smoke requires the sandbox."""
        workflow = (HERE.parents[1] / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        job = workflow[workflow.index("\n  cef-windows:\n"):workflow.index("\n  cef-windows-app:\n")]
        self.assertNotIn("unsandboxed-candidate", job.replace(".unsandboxedCandidate -ne $false", ""))
        self.assertIn("run: cargo build --locked --features windows-sandbox --lib --bin takosumi-browser-guard\n", job)
        self.assertIn('node scripts/stage-windows-cef.mjs --cef $cef[0].FullName --bin "$env:CARGO_TARGET_DIR\\debug" '
                      '--out "$env:RUNNER_TEMP\\w7-product"\n', job)
        self.assertIn("cargo build --locked --features fixture-inspection,windows-sandbox --lib\n", job)
        self.assertIn('node scripts/stage-windows-cef.mjs --fixture --cef $env:W7_CEF --bin "$env:CARGO_TARGET_DIR\\debug" '
                      '--out "$env:RUNNER_TEMP\\w7-fixture"\n', job)
        smokes = [line.strip() for line in job.splitlines() if "windows_smoke.py" in line]
        self.assertEqual(smokes, [
            'run: python -B native-tests/cef-candidate/windows_smoke.py --browser '
            '"$env:RUNNER_TEMP\\w7-product\\cef\\takosumi-cef-browser-process.exe" --work "$env:RUNNER_TEMP" '
            '--expect-sandbox',
            'run: python -B native-tests/cef-candidate/windows_smoke.py --browser '
            '"$env:RUNNER_TEMP\\w7-fixture\\cef\\takosumi-cef-browser-process.exe" --work "$env:RUNNER_TEMP" '
            '--expect-load --reuse --expect-sandbox',
        ])
        self.assertIn("cargo test --locked --features windows-sandbox --bin takosumi-browser-guard |", job)
        self.assertIn('$env:TAKOSUMI_WINDOWS_GUARD = "$env:RUNNER_TEMP\\w7-product\\cef\\takosumi-browser-guard.exe"\n',
                      job)
        self.assertIn('$env:TAKOSUMI_WINDOWS_BROWSER = '
                      '"$env:RUNNER_TEMP\\w7-product\\cef\\takosumi-cef-browser-process.exe"\n', job)
        self.assertIn('$exe = "$env:RUNNER_TEMP\\w7-product\\cef\\takosumi-cef-browser-process.exe"\n', job)
        self.assertIn("${{ runner.temp }}/sumi-cef-smoke-*/summary.json\n", job)
        # W7: the browser's child census tests run, every one of them.
        source = (HERE.parents[1] / "native-tests/cef-candidate/src/children_windows.rs").read_text(encoding="utf-8")
        tests = source.count("#[test]")
        self.assertIn("rustc --edition 2021 --test native-tests/cef-candidate/src/children_windows.rs -o $exe\n", job)
        self.assertIn(f"'^test result: ok\\. {tests} passed; 0 failed'", job)

    def test_the_macos_job_bundles_and_smokes_the_browser(self):
        """M1/M4: cef-macos runs standalone and packaged browser smokes
        with the seatbelt check and uploads text evidence only."""
        workflow = (HERE.parents[1] / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        job = workflow[workflow.index("\n  cef-macos:\n"):]
        self.assertRegex(job, r"(?m)^    runs-on: macos-latest$")
        # M4 adds packaged site flows. Comments between the fields must
        # not change which runner/bounded job this contract checks.
        self.assertRegex(job, r"(?m)^    timeout-minutes: 100$")
        self.assertIn("run: cargo build --locked --bin takosumi-cef-browser-process --bin takosumi-cef-browser-helper "
                      "--bin takosumi-browser-guard\n", job)
        self.assertIn('node scripts/bundle-macos-cef.mjs --cef "$cef" --bin "$CARGO_TARGET_DIR/debug" '
                      '--out "$RUNNER_TEMP/m1-product"\n', job)
        self.assertIn("cargo build --locked --features fixture-inspection --bin takosumi-cef-browser-process)\n", job)
        self.assertIn('node scripts/bundle-macos-cef.mjs --fixture --cef "$M1_CEF" --bin "$CARGO_TARGET_DIR/debug" '
                      '--out "$RUNNER_TEMP/m1-fixture"\n', job)
        app = "takosumi-cef-browser-process.app/Contents/MacOS/takosumi-cef-browser-process"
        smokes = [line.strip() for line in job.splitlines() if "windows_smoke.py" in line]
        self.assertEqual(smokes, [
            f'run: python3 -B native-tests/cef-candidate/windows_smoke.py --browser "$RUNNER_TEMP/m1-product/{app}" '
            '--work "$RUNNER_TEMP" --expect-sandbox',
            f'run: python3 -B native-tests/cef-candidate/windows_smoke.py --browser '
            f'"$CARGO_TARGET_DIR/debug/bundle/macos/Takosumi.app/Contents/Resources/cef/{app}" '
            '--work "$RUNNER_TEMP" --expect-sandbox',
            f'run: python3 -B native-tests/cef-candidate/windows_smoke.py --browser "$RUNNER_TEMP/m1-fixture/{app}" '
            '--work "$RUNNER_TEMP" --expect-load --reuse --expect-sandbox',
        ])
        upload = job[job.index("name: cef-macos-smoke"):]
        for path in ("sumi-cef-smoke-*/summary.json", "sumi-cef-smoke-*/*.log", "m1-product/bundle.json",
                     "m1-fixture/bundle.json"):
            self.assertIn(f"${{{{ runner.temp }}}}/{path}\n", upload)
        self.assertNotIn(".app\n", upload)
        paths = re.search(r"(?m)^          path: \|\n((?:            .+\n)+)", upload)
        self.assertIsNotNone(paths)
        entries = [entry.strip() for entry in paths.group(1).splitlines()]
        self.assertEqual([entry for entry in entries if entry.startswith("target-cef/")], [
            f"target-cef/debug/bundle/macos/{name}.app/Contents/Info.plist"
            for name in ("Takosumi", "TakosumiFixture")
        ])
        for entry in entries:
            # Packaged Info.plist is text; do not permit its containing app
            # or arbitrary target files when accepting these new paths.
            self.assertRegex(entry, r"\.(json|log|plist)$")
        self.assertNotIn("codesign", job)
        # The window wait's decisions run, every test of them.
        source = (HERE.parents[1] / "native-tests/cef-candidate/src/window_wait.rs").read_text(encoding="utf-8")
        self.assertIn('rustc --edition 2021 --test native-tests/cef-candidate/src/window_wait.rs -o "$exe"\n', job)
        self.assertIn(f"grep -q '^test result: ok\\. {source.count('#[test]')} passed; 0 failed'", job)

    def test_the_macos_job_runs_both_sites_with_and_without_packaged_resources(self):
        workflow = (HERE.parents[1] / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        job = workflow[workflow.index("\n  cef-macos:\n"):]
        flows = [line.strip() for line in job.splitlines()
                 if line.strip().startswith("run: python3 -B native-tests/desktop/macos_browser_process.py ")]
        unbundled = ('run: python3 -B native-tests/desktop/macos_browser_process.py '
                     '--binary "$CARGO_TARGET_DIR/debug/takosumi-mobile" '
                     '--browser "$RUNNER_TEMP/m1-fixture/takosumi-cef-browser-process.app/Contents/MacOS/'
                     'takosumi-cef-browser-process" --guard "$CARGO_TARGET_DIR/debug/takosumi-browser-guard" '
                     '--work "$RUNNER_TEMP" --site ')
        packaged = ('run: python3 -B native-tests/desktop/macos_browser_process.py --packaged '
                    '--binary "$M4_APP_BINARY" --work "$RUNNER_TEMP" --site ')
        self.assertEqual(flows, [prefix + site for prefix in (unbundled, packaged)
                                 for site in ("excalidraw", "photopea")])

    def test_the_macos_job_runs_the_guard_and_the_adapter_with_the_bundle(self):
        """M2: cef-macos runs every macOS guard self-test, then the adapter's
        macOS tests (the ignored ones included, the stand-in App skipped)
        with the built guard and the bundled product browser, requires each
        of them by name, and compiles the adapter's product and fixture
        builds; the desktop job's macOS run compiles the App with
        desktop-cef."""
        workflow = (HERE.parents[1] / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        job = workflow[workflow.index("\n  cef-macos:\n"):]
        # The guard's self-tests: exactly the cases its runner lists.
        source = (HERE.parents[1] / "native-tests/cef-candidate/src/guard_macos_tests.rs").read_text(encoding="utf-8")
        listed = re.search(r"const TESTS: &\[Test\] = tests!\[\n(.*?)\n\];", source, re.S)
        self.assertIsNotNone(listed, "the macOS guard's runner lists its cases")
        cases = re.findall(r"^\s+([a-z_]+),$", listed.group(1), re.M)
        self.assertEqual(len(cases), len(set(cases)))
        for case in cases:
            self.assertIn(f"fn {case}() {{", source)
        guard = job[job.index("- name: Guard self-tests (macOS)"):]
        guard = guard[:guard.index("\n      - name:")]
        self.assertIn('cargo test --locked --bin takosumi-browser-guard | tee "$log"\n', guard)
        self.assertIn('test "${PIPESTATUS[0]}" = 0 ||', guard)
        self.assertIn(f"grep -q '^test result: ok\\. {len(cases)} passed; 0 failed$' \"$log\"", guard)
        # The adapter's macOS tests: the built guard and the bundled browser,
        # every test of the module by name and the exact count.
        adapter = job[job.index("- name: App adapter with the built guard and bundled browser (Process API)"):]
        adapter = adapter[:adapter.index("\n      - name:")]
        self.assertIn('export TAKOSUMI_MACOS_GUARD="$CARGO_TARGET_DIR/debug/takosumi-browser-guard"\n', adapter)
        self.assertIn('export TAKOSUMI_MACOS_BROWSER="$RUNNER_TEMP/m1-product/takosumi-cef-browser-process.app/'
                      'Contents/MacOS/takosumi-cef-browser-process"\n', adapter)
        self.assertIn("if: ${{ !cancelled() && steps.bundle.outcome == 'success' && "
                      "steps.plugin-lock.outcome == 'success' }}", adapter)
        self.assertIn("cargo test --locked --manifest-path src-tauri/plugins/app-screen/Cargo.toml --features "
                      "desktop-process-experiment --lib -- macos_tests --include-ignored "
                      "--skip app_stand_in_for_the_death_case --test-threads 1 "
                      '--show-output | tee "$log"\n', adapter)
        self.assertNotIn("fixture-overrides", adapter)
        plugin = (HERE.parents[1] / "src-tauri/plugins/app-screen/src/desktop/process_adapter.rs").read_text(
            encoding="utf-8")
        module = plugin[plugin.index("\nmod macos_tests {\n"):]
        tests = re.findall(r"^    #\[test\]\n(?:    #\[ignore = \"[^\"]*\"\]\n)?    fn ([a-z_]+)\(\)", module, re.M)
        self.assertEqual(len(tests), module.count("#[test]"))
        # The stand-in App runs only inside the death case; the run skips it
        # (a substring filter that no other test name contains).
        self.assertIn("app_stand_in_for_the_death_case", tests)
        tests.remove("app_stand_in_for_the_death_case")
        self.assertFalse([name for name in tests if "app_stand_in_for_the_death_case" in name])
        self.assertIn('"desktop::process_adapter::macos_tests::app_stand_in_for_the_death_case",\n', module)
        required = re.findall(r"^\s+desktop::process_adapter::macos_tests::([a-z_]+)(?: \\|;)", adapter, re.M)
        self.assertEqual(sorted(required), sorted(tests))
        self.assertEqual(len(required), len(set(required)))
        # The same macos_tests filter also includes the executable identity
        # module. Require every one of those tests, not just a larger total.
        image = (HERE.parents[1] / "src-tauri/plugins/app-screen/src/desktop/process_image.rs").read_text(
            encoding="utf-8")
        image_tests = re.findall(r"^    #\[test\]\n    fn ([a-z_]+)\(\)", image, re.M)
        self.assertTrue(image_tests)
        self.assertEqual(len(image_tests), image.count("#[test]"))
        image_required = re.findall(r"^\s+desktop::process_macos::image::macos_tests::([a-z_]+)(?: \\|;)",
                                    adapter, re.M)
        self.assertEqual(sorted(image_required), sorted(image_tests))
        self.assertEqual(len(image_required), len(set(image_required)))
        self.assertIn('grep -qF "test $name ... ok" "$log"', adapter)
        self.assertIn(f"grep -q '^test result: ok\\. {len(tests) + len(image_tests)} passed; 0 failed; 0 ignored' \"$log\"",
                      adapter)
        # The socket check uses the browser bundle's identifier, as the
        # bundler writes it.
        bundler = (HERE.parents[1] / "scripts/bundle-macos-cef.mjs").read_text(encoding="utf-8")
        identifier = re.search(r'^export const IDENTIFIER = "([^"]+)";$', bundler, re.M)
        self.assertIsNotNone(identifier)
        backend = (HERE.parents[1] / "src-tauri/plugins/app-screen/src/desktop/process_macos.rs").read_text(
            encoding="utf-8")
        self.assertIn(f'const BROWSER_BUNDLE_ID: &str = "{identifier.group(1)}";\n', backend)
        # The real opener is never started: the tests' open_externally only
        # records the link.
        self.assertIn("#[cfg(test)]\nfn open_externally(url: &str) {\n    OPENED.lock().unwrap().push(url.to_owned());\n}",
                      plugin)
        # The adapter's own lock comes from the App's, pruned only.
        lock = job[job.index("- name: Plugin lockfile from the App's"):]
        lock = lock[:lock.index("\n      - name:")]
        self.assertIn("id: plugin-lock\n", lock)
        self.assertIn("cp src-tauri/Cargo.lock src-tauri/plugins/app-screen/Cargo.lock\n", lock)
        self.assertIn("added = sorted(plugin - app)", lock)
        # The product and fixture builds compile outside the tests.
        compiled = job[job.index("- name: Compile the adapter's macOS product and fixture builds"):]
        compiled = compiled[:compiled.index("\n      - name:")]
        self.assertIn("if: ${{ !cancelled() && steps.plugin-lock.outcome == 'success' }}", compiled)
        for features in ("desktop-process-experiment", "fixture-overrides"):
            self.assertIn("cargo check --locked --manifest-path src-tauri/plugins/app-screen/Cargo.toml "
                          f"--features {features} --lib\n", compiled)
        desktop = workflow[workflow.index("\n  desktop:\n"):workflow.index("\n  cef-windows:\n")]
        cef_app = desktop[desktop.index("- name: Compile desktop App with the per-profile CEF adapter"):]
        cef_app = cef_app[:cef_app.index("\n      - name:")]
        self.assertIn("if: runner.os == 'Windows' || runner.os == 'macOS'\n", cef_app)
        self.assertIn("cargo check --locked --manifest-path src-tauri/Cargo.toml --features desktop-cef\n", cef_app)
        upload = job[job.index("name: cef-macos-smoke"):]
        for path in ("m2-*-tests.log", "sumi-m2-*/profiles/*/browser.log", "sumi-m2-*/profiles/*/cef-native.log",
                     "sumi-m2-*/rt/*/guard.json"):
            self.assertIn(f"${{{{ runner.temp }}}}/{path}\n", upload)

    def test_the_app_runs_take_a_staged_sandboxed_browser(self):
        """W7: the W3 job and the W4 real-site workflow stage the sandboxed
        fixture browser and hand the App those files."""
        root = HERE.parents[1] / ".github/workflows"
        workflow = (root / "ci.yml").read_text(encoding="utf-8")
        w3 = workflow[workflow.index("\n  cef-windows-app:\n"):workflow.index("\n  cef-windows-package:\n")]
        w4 = (root / "windows-real-sites.yml").read_text(encoding="utf-8")
        for job, stage, runs in ((w3, "w3-stage", 2), (w4, "w4-stage", 2)):
            self.assertNotIn("unsandboxed-candidate", job)
            self.assertIn("cargo build --locked --features fixture-inspection,windows-sandbox --lib "
                          "--bin takosumi-browser-guard\n", job)
            self.assertIn('node scripts/stage-windows-cef.mjs --fixture --cef $cef[0].FullName '
                          f'--bin "$env:CARGO_TARGET_DIR\\debug" --out "$env:RUNNER_TEMP\\{stage}"\n', job)
            lines = [line for line in job.splitlines() if "windows_browser_process.py --binary" in line]
            self.assertEqual(len(lines), runs)
            for line in lines:
                self.assertIn('--browser "$stage\\takosumi-cef-browser-process.exe" '
                              '--guard "$stage\\takosumi-browser-guard.exe" ', line)
            self.assertEqual(job.count(f"$stage = Join-Path $env:RUNNER_TEMP '{stage}\\cef'\n"), runs)
            self.assertIn("key: cef-windows-app-cargo-w7-", job)

    def test_recovery_is_a_fresh_case_with_six_required_cases_and_its_fault_source_pinned(self):
        workflow = (HERE.parents[1] / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        job = workflow[workflow.index("\n  cef-windows-app:\n"):workflow.index("\n  cef-windows-package:\n")]
        start = job.index("      - name: Actual home recovery after an owned browser fault\n")
        step = job[start:job.index("      - name: Remove the fixture CA\n", start)]
        self.assertIn("steps.w3-tls.outcome == 'success'", step)
        self.assertIn("'sumi-w3-recover'", step)
        command = next(line for line in step.splitlines() if line.strip().startswith("python -B "))
        self.assertIn(" --recover | Tee-Object", command)
        for other_mode in ("--packaged", "--real-site", "--restart", "--reset", "--unsandboxed-candidate"):
            self.assertNotIn(other_mode, command)
        cases = re.search(r"\$recoveryCases = @\((.*?)\n          \)", step, re.S)
        self.assertIsNotNone(cases)
        self.assertEqual(re.findall(r"'([a-z-]+)'", cases.group(1)),
                         list(runner.OBSERVED_CASES["recover"]) + list(runner.FINAL_CASES))
        self.assertIn(".Count -ne 6", step)
        self.assertIn("$summary.phases[0].phase -ne 'recover'", step)
        self.assertIn("native-tests/desktop/windows_browser_fault.py", runner.WINDOWS_SOURCES)

    def test_recovery_final_sandbox_evidence_must_name_the_replacement_pid(self):
        opened = {"processes": [{"profile": "work", "opened": {"nativePid": 30}}]}
        record = {"profile": "work", "stage": "recovery-final", "browser": {"pid": 29},
                  "renderers": [], "problems": []}
        with self.assertRaises(Failure):
            runner.sandbox_summary({"sandbox": [record]}, opened, True, ("recovery-final",))
        record["browser"]["pid"] = 30
        self.assertEqual(len(runner.sandbox_summary({"sandbox": [record]}, opened, True,
                                                   ("recovery-final",))), 1)

    def test_recovery_rejects_combined_or_unsandboxed_modes_before_creating_the_case(self):
        import subprocess
        with tempfile.TemporaryDirectory() as root:
            case = Path(root) / "not-created"
            command = [sys.executable, "-B", str(HERE / "windows_browser_process.py"),
                       "--binary", "unused.exe", "--browser", "unused-browser.exe", "--guard", "unused-guard.exe",
                       "--data", str(case), "--tls-cert", "unused.cert", "--tls-key", "unused.key",
                       "--tls-ca", "unused.ca", "--recover"]
            for mode in (["--restart"], ["--reset"], ["--packaged"], ["--real-site", "photopea"],
                         ["--unsandboxed-candidate"]):
                result = subprocess.run(command + mode, capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn("standalone sandboxed synthetic-site scenario", result.stderr)
                self.assertFalse(case.exists())

    def test_the_sandboxed_browsers_dll_is_hashed_with_it(self):
        import tempfile
        with tempfile.TemporaryDirectory() as directory:
            browser = Path(directory) / "takosumi-cef-browser-process.exe"
            browser.write_bytes(b"MZ")
            self.assertEqual(runner.browser_code(browser), [])
            browser.with_suffix(".dll").write_bytes(b"MZ")
            self.assertEqual(runner.browser_code(browser), [Path(directory) / "takosumi-cef-browser-process.dll"])

    def test_the_real_site_workflow_requires_exactly_the_runner_cases(self):
        workflow = (HERE.parents[1] / ".github/workflows/windows-real-sites.yml").read_text(encoding="utf-8")
        for site in sorted(runner.REAL_SITES):
            block = re.search(r"\$%sCases = @\(\n(.*?)\n\s*\)" % site, workflow, re.S)
            count = re.search(r"\$%sCount -ne (\d+)\)" % site, workflow)
            self.assertIsNotNone(block, f"The real-site workflow lists the {site} cases")
            self.assertIsNotNone(count, f"The real-site workflow counts the {site} cases")
            cases = runner.real_site_cases(site)
            self.assertEqual(re.findall(r"'([a-z/-]+)'", block.group(1)), cases)
            self.assertEqual(int(count.group(1)), len(cases))

    def test_the_guards_forced_cleanup_lines_are_collected(self):
        ours = ('{"guardPid":41,"forced":2,"processes":[{"pid":7,"exit":"0xE0007462","image":"x.exe",'
                '"type":"utility","utilitySubType":"network.mojom.NetworkService"}],"enteredAfterList":1,'
                '"endedByThemselves":3,"identifyMs":4}')
        earlier = '{"guardPid":40,"forced":0,"processes":[],"enteredAfterList":0,"endedByThemselves":5,"identifyMs":2}'
        text = "\n".join(["[1:2:ERROR:x.cc(1)] unrelated", runner.GUARD_DIAGNOSTIC + earlier,
                          runner.GUARD_DIAGNOSTIC + ours, "partial " + runner.GUARD_DIAGNOSTIC + "{not json", ""])
        found = runner.guard_diagnostics(text, 41)
        self.assertEqual([entry.get("guardPid") for entry in found], [41, None])
        self.assertEqual(found[0]["processes"][0]["utilitySubType"], "network.mojom.NetworkService")
        self.assertEqual(found[1], {"unparsed": "{not json"})
        self.assertEqual([entry.get("guardPid") for entry in runner.guard_diagnostics(text)], [40, 41, None])
        self.assertEqual(runner.guard_diagnostics("nothing here\n", 41), [])
        # The same prefix as the guard writes.
        rust = (HERE.parents[1] / "native-tests/cef-candidate/src/guard_windows_identity.rs").read_text(
            encoding="utf-8")
        self.assertIn(f'pub(super) const FORCED_DIAGNOSTIC: &str = "{runner.GUARD_DIAGNOSTIC}";', rust)
        self.assertIn('"guardPid": std::process::id(),', rust)

    def test_the_browsers_shutdown_lines_are_collected(self):
        """W7: the browser's own line after CefShutdown (children still
        running, how it ends, its exit code), attributed by its pid."""
        import windows_smoke as smoke
        ours = ('{"pid":9212,"children":46,"unopened":0,"end":"exit-process","exitCode":0,'
                '"closeMs":23810,"shutdownMs":2140,"totalMs":26020}')
        earlier = ('{"pid":5000,"children":4,"unopened":0,"end":"exit-process","exitCode":0,'
                   '"closeMs":null,"shutdownMs":800,"totalMs":null}')
        text = "\n".join(["DevTools listening on ws://127.0.0.1:1/x", smoke.BROWSER_SHUTDOWN + earlier,
                          smoke.BROWSER_SHUTDOWN + ours, smoke.BROWSER_SHUTDOWN + "{cut", ""])
        found = smoke.browser_shutdown(text, 9212)
        self.assertEqual(found, [json.loads(ours), {"unparsed": "{cut"}])
        self.assertEqual(len(smoke.browser_shutdown(text)), 3)
        self.assertEqual(smoke.browser_shutdown("nothing\n", 9212), [])
        records = [{"profile": "work", "receipt": {"rootExit": 0}, "browserShutdown": [json.loads(ours)]},
                   {"profile": "personal", "receipt": "unreadable: x", "browserShutdown": []}]
        self.assertEqual(runner.shutdown_summary(records), [
            {"profile": "work", "rootExit": 0, "browser": json.loads(ours)},
            {"profile": "personal", "rootExit": None, "browser": None},
        ])
        self.assertEqual(runner.shutdown_summary(None), [])
        # The same prefix and fields as the browser writes.
        rust = (HERE.parents[1] / "native-tests/cef-candidate/src/children_windows.rs").read_text(encoding="utf-8")
        self.assertIn(f'pub const EVIDENCE_PREFIX: &str = "{smoke.BROWSER_SHUTDOWN}";', rust)
        for field in ("pid", "children", "unopened", "end", "exitCode", "closeMs", "shutdownMs", "totalMs"):
            self.assertIn(f'\\"{field}\\":', rust)
        # The moments the durations come from are marked where they happen.
        for marked in ("Mark::CloseRequested", "Mark::BrowserClosed", "Mark::Quit"):
            self.assertIn(f"children_windows::mark(children_windows::{marked});", main_source())
        self.assertIn('End::ExitProcess => "exit-process"', rust)
        self.assertIn("win32::ExitProcess(exit_code as u32)", rust)
        # After CefShutdown: the sandboxed browser records and ends with
        # ExitProcess; the unsandboxed candidate records and returns.
        main = (HERE.parents[1] / "native-tests/cef-candidate/src/process_main.rs").read_text(encoding="utf-8")
        ending = ("    let code = i32::from(failed);\n    #[cfg(windows)]\n"
                  "    children_windows::finish(code, cfg!(feature = \"windows-sandbox\"));\n    code\n}")
        self.assertIn(ending, main)
        # macOS may record CefShutdown's return between these operations.
        # Windows must still finish only after CefShutdown has returned.
        self.assertLess(main.index("\n    shutdown();"), main.index(ending))

    def test_a_packaged_run_lets_the_app_resolve_its_bundle(self):
        class Args:
            site, real_site, packaged = "https://127.0.0.1:1", None, True
            browser, guard = Path("C:/b.exe"), Path("C:/g.exe")
        base = {"PATH": "p", "TAKOSUMI_BROWSER_PROCESS_PROGRAM": "inherited", "WebView2_X": "1"}
        session = Path("C:/case/sumi-w6")
        env = runner.browser_env(base, Args, session, session / "external-open.log")
        for key in ("TAKOSUMI_BROWSER_PROCESS_PROGRAM", "TAKOSUMI_BROWSER_PROCESS_GUARD",
                    "TAKOSUMI_BROWSER_PROCESS_RUNTIME", "WebView2_X"):
            self.assertNotIn(key, env)
        self.assertEqual((env["PATH"], env["TAKOSUMI_BROWSER_PROCESS_INSPECT"],
                          env["TAKOSUMI_BROWSER_PROCESS_CASE"]), ("p", "1", str(session)))
        Args.packaged = False
        env = runner.browser_env(base, Args, session, session / "external-open.log")
        self.assertEqual((env["TAKOSUMI_BROWSER_PROCESS_PROGRAM"], env["TAKOSUMI_BROWSER_PROCESS_RUNTIME"]),
                         (str(Args.browser), str(session / "r")))
        installed = Path("C:/Users/runner/install/Takosumi.exe")
        self.assertEqual(runner.packaged_files(installed),
                         (installed.parent / "cef" / "takosumi-cef-browser-process.exe",
                          installed.parent / "cef" / "takosumi-browser-guard.exe"))
        # The same names the App resolves (process_setup.rs).
        rust = (HERE.parents[1] / "src-tauri/plugins/app-screen/src/desktop/process_setup.rs").read_text(
            encoding="utf-8")
        self.assertIn(f'pub(super) const BROWSER: &str = "{runner.PACKAGED_BROWSER}";', rust)
        self.assertIn(f'pub(super) const GUARD: &str = "{runner.PACKAGED_GUARD}";', rust)
        self.assertIn('pub(super) const PACKAGED: &str = "cef";', rust)

    def test_a_packaged_process_must_run_from_the_installed_cef(self):
        cef = "D:\\a\\_temp\\w6-fixture-app\\cef"
        self.assertTrue(observer.inside("D:\\a\\_temp\\w6-fixture-app\\cef\\takosumi-browser-guard.exe", cef))
        self.assertTrue(observer.inside("d:\\A\\_TEMP\\w6-fixture-app\\CEF\\takosumi-cef-browser-process.exe", cef))
        for image in ("D:\\a\\target-w3\\debug\\takosumi-browser-guard.exe",
                      "D:\\a\\_temp\\w6-fixture-app\\cef-other\\x.exe",
                      "D:\\a\\_temp\\w6-fixture-app\\cef", None):
            self.assertFalse(observer.inside(image, cef), image)

    def test_installed_processes_load_no_crt_and_the_installed_libcef(self):
        cef = "D:\\a\\_temp\\w6-fixture-app\\cef"
        system = ["C:\\Windows\\SYSTEM32\\ntdll.dll", "C:\\Windows\\System32\\KERNEL32.DLL"]
        browser = [cef + "\\takosumi-cef-browser-process.exe", cef + "\\libcef.dll", *system]
        self.assertEqual(observer.module_problems("browser", browser, cef), [])
        self.assertEqual(observer.module_problems("guard", [cef + "\\takosumi-browser-guard.exe", *system], cef), [])
        crt = "C:\\Windows\\System32\\VCRUNTIME140.dll"
        self.assertEqual(observer.module_problems("guard", [*system, crt], cef), [f"the guard loaded {crt}"])
        self.assertEqual(len(observer.module_problems("browser", [*browser, "C:\\x\\msvcp140_1.dll"], cef)), 1)
        elsewhere = "D:\\a\\target-w3\\debug\\libcef.dll"
        self.assertEqual(observer.module_problems("browser", [cef + "\\b.exe", elsewhere, *system], cef),
                         [f"the browser loaded libcef.dll from {elsewhere}"])
        self.assertEqual(observer.module_problems("browser", system, cef), ["the browser has not loaded libcef.dll"])
        self.assertEqual(observer.module_problems("guard", None, cef), ["no module snapshot of the guard"])
        # W7: the bootstrap must have loaded the installed browser DLL.
        dll = observer.BROWSER_DLL
        self.assertEqual(observer.module_problems("browser", [*browser, cef + "\\" + dll], cef, dll), [])
        self.assertEqual(observer.module_problems("browser", browser, cef, dll), [f"the browser has not loaded {dll}"])
        other = "D:\\a\\target-w3\\debug\\" + dll
        self.assertEqual(observer.module_problems("browser", [*browser, other], cef, dll),
                         [f"the browser loaded {dll} from {other}"])
        # MODULEENTRY32W: 1080 bytes on x64 Windows (two-byte WCHAR).
        ctypes = observer.ctypes
        if ctypes.sizeof(ctypes.c_void_p) == 8:
            self.assertEqual(ctypes.sizeof(observer.ModuleEntry), 48 + 516 * ctypes.sizeof(ctypes.c_wchar))

    def test_each_quit_case_names_who_closed_its_dialog(self):
        class Args:
            real_site = "photopea"
        observation = {"evidence": {"quit-cancel-stops": {"dialogClosedBy": "observer"},
                                    "quit-partial-order": {"dialogClosedBy": "browser"}}}
        self.assertEqual(runner.dialog_lines(Args, observation),
                         ["DIALOG closedBy=observer case=photopea/quit-cancel-stops",
                          "DIALOG closedBy=browser case=photopea/quit-partial-order"])
        Args.real_site = None
        self.assertEqual(runner.dialog_lines(Args, observation)[0], "DIALOG closedBy=observer case=quit-cancel-stops")
        for value in (None, "page"):
            broken = {"evidence": dict(observation["evidence"], **{"quit-partial-order": {"dialogClosedBy": value}})}
            with self.assertRaises(Failure):
                runner.dialog_lines(Args, broken)

    def test_the_real_site_workflow_flags_a_cancel_by_the_page_or_browser(self):
        workflow = (HERE.parents[1] / ".github/workflows/windows-real-sites.yml").read_text(encoding="utf-8")
        cases = "|".join(runner.DIALOG_CASES)
        for site in sorted(runner.REAL_SITES):
            title = site.capitalize()
            self.assertIn(f"${site}Dialogs = @(Select-String -Path $log -Pattern "
                          f"'^DIALOG closedBy=(observer|browser) case={site}/({cases})$')", workflow)
            self.assertIn(f"if (${site}Dialogs.Count -ne {len(runner.DIALOG_CASES)}) {{ throw", workflow)
            self.assertIn(f"foreach ($dialog in ${site}Dialogs) {{\n"
                          f"            if ($dialog.Line -match ' closedBy=browser ') {{ Write-Output "
                          f"\"::warning title={title} beforeunload closed by the page or browser::", workflow)

    def test_the_real_site_workflow_runs_only_when_asked(self):
        workflow = (HERE.parents[1] / ".github/workflows/windows-real-sites.yml").read_text(encoding="utf-8")
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
                      "    # A dispatch is keyed on its input, not the commit it resolves to: a job's\n"
                      "    # group is fixed when it is queued, before any of its steps runs (keying\n"
                      "    # on the resolved commit would take a separate resolving job).\n"
                      "    concurrency:\n"
                      f"      group: takosumi-mobile-windows-real-sites-${{{{ {test_ref} }}}}\n"
                      "      cancel-in-progress: false\n", workflow)
        self.assertRegex(workflow, r"(?m)^permissions:\n  contents: read$")

    def test_a_dispatch_tests_only_this_repositorys_commits(self):
        workflow = (HERE.parents[1] / ".github/workflows/windows-real-sites.yml").read_text(encoding="utf-8")
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
        refs = [dict(re.findall(r"^  (MOBILE_KIT_SOURCE_REF|TAKOSUMI_SOURCE_REF): (\S+)$",
                                (root / name).read_text(encoding="utf-8"), re.M))
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
                            (tests / "browser_process.rs").read_text(encoding="utf-8"))
        watchdog = re.search(r"from_secs\(if real_site\(\) \{ (\d+) \} else \{ (\d+) \}\)",
                             (tests / "browser_process_windows.rs").read_text(encoding="utf-8"))
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
        probe = (HERE / "browser-process.mjs").read_text(encoding="utf-8")
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
