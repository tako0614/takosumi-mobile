#!/usr/bin/env python3
"""Observe the actual App's CEF browsers on Windows (windows_browser_process.py).

The Windows counterpart of browser-process.mjs; the runner starts it once per
App launch. Python 3 standard library only: ctypes for Win32 (windows, the
foreground, the TCP table), a minimal RFC 6455 client for the browsers'
DevTools endpoints. No replacement shell IPC: the App's own home drives the
browsers; this reads their pages and windows and answers at the checkpoints
the App waits for (the marker files of browser_process.rs).

Each browser's loopback inspection port (cef-candidate `fixture-inspection`)
must be the one its `inspection.json` names, listened on by exactly the App's
attested native browser process. Pages are the runner's local fixture site.
State is synthetic (a nonce in localStorage and a script cookie, and the
site's HttpOnly session cookie set by the sign-in stand-in); no real login.

W7: once each profile's page has loaded, its browser and every renderer
below it in its process tree (which runs in its guard's job inside the
App's, with Chromium's own jobs below) must show Chromium's sandbox, read
with windows_smoke.py's sandbox_evidence: at least one renderer, each at
Untrusted or Low integrity below the browser's, no `--no-sandbox`, no live
process whose command line could not be read. A real-site run (W4) checks
again once the real site is ready in both profiles, so its own renderers
(site isolation, ad frames) are covered. The records go into the
observation (`sandbox`, each with its `stage`: attach or real-site); with
--unsandboxed-candidate they are recorded, not required.

Window switching uses SetForegroundWindow (then, if Windows refuses, the
task switchers' AttachThreadInput route), and Work is minimized before the
home reopens it with ShowWindowAsync. It never sends keyboard or mouse
input. Beforeunload dialogs are cancelled through DevTools
(Page.handleJavaScriptDialog), not with a native key as on Linux; native
window closes are WM_CLOSE, as a click on the title bar's close button.
"""
import argparse
import base64
import ctypes
import hashlib
import http.client
import json
import ntpath
import os
import socket
import struct
import sys
import time
from pathlib import Path
from urllib.parse import urlsplit

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "cef-candidate"))
from windows_smoke import Failure, ProcessTree, require, sandbox_evidence  # noqa: E402
from windows_browser_fault import FAULT_EXIT, OwnedBrowser, WindowsProcesses, recovery_paths  # noqa: E402

GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
MAX_MESSAGE = 16 * 1024 * 1024
MARKER_WAIT = 30
SETTLE_WAIT = 10
# One pure read's bound through a starved DevTools channel; the same order
# of storm the driver's 60s cancel-inspected bound already waits out.
STARVED_READ_WAIT = 60
# A live storm can outlast the base bound while the event stream keeps
# arriving (run 43173821786: DOMStorage.enable starved the full 60 s under
# ~90 s of observed renderer churn on the real site). A stream that is
# still receiving events names a STARVED channel, not a dead one, so the
# bound then extends in quiet-window steps while events keep landing, up to
# the absolute cap; a channel that goes quiet fails at its own bound.
STARVED_QUIET_WAIT = 15
STARVED_MAX_WAIT = 240
# The same bound for the dialog-cancel send loop: each timed-out send is
# re-asked while the profile lives, until the shared storm bound.
STARVED_SEND_WAIT = 60
# The fixture site's documents: a navigation within a profile, or the boot
# load of a new browser (the cef-candidate popup probe's 25 s load bound).
LOAD_WAIT = 25
# W7: how long a loaded profile's browser may take to show a renderer.
SANDBOX_WAIT = 15
# Windows: a profile's browser and guard ending after its last window closed
# (the Linux observer allows 10 s; process teardown is slower here).
END_WAIT = 20
RAISE_REFUSED = "Reused browser window was not raised:"
# W4: real third-party sites behind the fixture home's app link. A site is
# ready only once its editor has rendered, which neither a browser error page
# (another origin) nor the site's own shell satisfies: Excalidraw's canvas
# without its crash screen (`ErrorSplash`, which also carries the `.excalidraw`
# class), Photopea's editor start screen. Photopea first serves a static
# landing page (shown even if the editor's code never loads), so the editor is
# entered from it with its start button (`landing`, `enter`), as the Linux
# probe does (browser-process.mjs). Anonymous only: nothing signs in, no
# credential is typed.
REAL_SITES = {
    "excalidraw": {"url": "https://excalidraw.com/", "origin": "https://excalidraw.com",
                   "ready": "Boolean(document.querySelector('.excalidraw canvas') "
                            "&& !document.querySelector('.ErrorSplash'))"},
    "photopea": {"url": "https://www.photopea.com/", "origin": "https://www.photopea.com",
                 "ready": "Boolean(document.body?.innerText.includes('Open From Computer'))",
                 "landing": "Boolean(document.body?.innerText.includes('Start using Photopea'))",
                 "enter": "(()=>{const start=[...document.querySelectorAll('button')].find(e=>"
                          "e.innerText.trim()==='Start using Photopea'&&!e.disabled);"
                          "if(!start)throw Error('No Photopea start button');start.click();return true})()"},
}
# Each stage of a real site in each profile (both profiles load at once): its
# document (with a landing: landing or editor), then, from a landing, its editor.
REAL_SITE_WAIT = 60
# After the final quit request, a remaining real-site page may still ask its
# own beforeunload; accept it within this bound so the quit can finish.
FINAL_QUIT_WAIT = 45
GA_ROOT = 2
# ShowWindow: minimized without activating another window (the home stays in
# front for the reopen).
SW_SHOWMINNOACTIVE = 7
GW_HWNDNEXT = 2
GWL_EXSTYLE = -20
WS_EX_TOPMOST = 0x8
# After Personal is switched to the front, how long Work's visibility is
# sampled (every OCCLUSION_INTERVAL) for an occlusion update to show up.
OCCLUSION_WATCH = 1.5
OCCLUSION_INTERVAL = 0.1
# How long one foreground change may take to show (each window switch tries
# twice: SetForegroundWindow, then joined to the foreground thread's input).
FOREGROUND_WAIT = 2.0
# Bound on the top-level windows walked for the Z-order evidence.
Z_ORDER_LIMIT = 4096
WM_CLOSE = 0x0010
SPI_GETFOREGROUNDLOCKTIMEOUT = 0x2000
AF_INET = 2
TCP_TABLE_OWNER_PID_LISTENER = 3
MIB_TCP_STATE_LISTEN = 2
ERROR_INSUFFICIENT_BUFFER = 122


class Rect(ctypes.Structure):  # RECT
    _fields_ = [("left", ctypes.c_int32), ("top", ctypes.c_int32), ("right", ctypes.c_int32),
                ("bottom", ctypes.c_int32)]


class Placement(ctypes.Structure):  # WINDOWPLACEMENT
    _fields_ = [("length", ctypes.c_uint32), ("flags", ctypes.c_uint32), ("showCmd", ctypes.c_uint32),
                ("minX", ctypes.c_int32), ("minY", ctypes.c_int32), ("maxX", ctypes.c_int32),
                ("maxY", ctypes.c_int32), ("normal", Rect)]


class GuiThreadInfo(ctypes.Structure):  # GUITHREADINFO
    _fields_ = [("cbSize", ctypes.c_uint32), ("flags", ctypes.c_uint32), ("hwndActive", ctypes.c_void_p),
                ("hwndFocus", ctypes.c_void_p), ("hwndCapture", ctypes.c_void_p),
                ("hwndMenuOwner", ctypes.c_void_p), ("hwndMoveSize", ctypes.c_void_p),
                ("hwndCaret", ctypes.c_void_p), ("rcCaret", Rect)]


class LastInput(ctypes.Structure):  # LASTINPUTINFO
    _fields_ = [("cbSize", ctypes.c_uint32), ("dwTime", ctypes.c_uint32)]


# --- Win32 -----------------------------------------------------------------

class ModuleEntry(ctypes.Structure):  # MODULEENTRY32W
    _fields_ = [("dwSize", ctypes.c_uint32), ("th32ModuleID", ctypes.c_uint32), ("th32ProcessID", ctypes.c_uint32),
                ("GlblcntUsage", ctypes.c_uint32), ("ProccntUsage", ctypes.c_uint32),
                ("modBaseAddr", ctypes.c_void_p), ("modBaseSize", ctypes.c_uint32), ("hModule", ctypes.c_void_p),
                ("szModule", ctypes.c_wchar * 256), ("szExePath", ctypes.c_wchar * 260)]


class Win32:
    """user32, kernel32 and iphlpapi with the prototypes used here."""

    def __init__(self):
        handle, boolean, dword = ctypes.c_void_p, ctypes.c_int, ctypes.c_uint32
        self.user32 = ctypes.WinDLL("user32", use_last_error=True)
        self.kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        self.iphlpapi = ctypes.WinDLL("iphlpapi", use_last_error=True)
        for library, name, restype, argtypes in (
            (self.user32, "GetWindowThreadProcessId", dword, [handle, ctypes.POINTER(dword)]),
            (self.user32, "IsWindow", boolean, [handle]),
            (self.user32, "IsWindowVisible", boolean, [handle]),
            (self.user32, "IsIconic", boolean, [handle]),
            (self.user32, "ShowWindowAsync", boolean, [handle, ctypes.c_int]),
            (self.user32, "GetAncestor", handle, [handle, ctypes.c_uint]),
            (self.user32, "GetForegroundWindow", handle, []),
            (self.user32, "SetForegroundWindow", boolean, [handle]),
            (self.user32, "BringWindowToTop", boolean, [handle]),
            (self.user32, "AttachThreadInput", boolean, [dword, dword, boolean]),
            (self.user32, "PostMessageW", boolean, [handle, ctypes.c_uint, ctypes.c_size_t, ctypes.c_ssize_t]),
            (self.user32, "SystemParametersInfoW", boolean, [ctypes.c_uint, ctypes.c_uint, ctypes.c_void_p,
                                                             ctypes.c_uint]),
            (self.user32, "GetWindowPlacement", boolean, [handle, ctypes.POINTER(Placement)]),
            (self.user32, "GetWindowRect", boolean, [handle, ctypes.POINTER(Rect)]),
            (self.user32, "GetTopWindow", handle, [handle]),
            (self.user32, "GetWindow", handle, [handle, ctypes.c_uint]),
            (self.user32, "GetWindowLongPtrW", ctypes.c_ssize_t, [handle, ctypes.c_int]),
            (self.user32, "GetLastInputInfo", boolean, [ctypes.POINTER(LastInput)]),
            (self.user32, "GetGUIThreadInfo", boolean, [dword, ctypes.POINTER(GuiThreadInfo)]),
            (self.user32, "GetClassNameW", ctypes.c_int, [handle, ctypes.c_wchar_p, ctypes.c_int]),
            (self.kernel32, "GetTickCount", dword, []),
            (self.kernel32, "OpenProcess", handle, [dword, boolean, dword]),
            (self.kernel32, "CloseHandle", boolean, [handle]),
            (self.kernel32, "QueryFullProcessImageNameW", boolean, [handle, dword, ctypes.c_wchar_p,
                                                                   ctypes.POINTER(dword)]),
            (self.kernel32, "CreateToolhelp32Snapshot", handle, [dword, dword]),
            (self.kernel32, "Module32FirstW", boolean, [handle, ctypes.POINTER(ModuleEntry)]),
            (self.kernel32, "Module32NextW", boolean, [handle, ctypes.POINTER(ModuleEntry)]),
            (self.kernel32, "GetCurrentThreadId", dword, []),
            (self.iphlpapi, "GetExtendedTcpTable", dword, [ctypes.c_void_p, ctypes.POINTER(dword), boolean,
                                                           ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]),
        ):
            function = getattr(library, name)
            function.restype, function.argtypes = restype, argtypes

    def image(self, pid):
        """A running process's executable path, or None."""
        process = self.kernel32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
        if not process:
            return None
        try:
            name = ctypes.create_unicode_buffer(32768)
            size = ctypes.c_uint32(32768)
            if not self.kernel32.QueryFullProcessImageNameW(process, 0, name, ctypes.byref(size)):
                return None
            return name.value
        finally:
            self.kernel32.CloseHandle(process)

    def modules(self, pid):
        """The paths of the modules a running process has loaded (a
        TH32CS_SNAPMODULE snapshot, retried while the process is still
        settling); None if no snapshot could be taken."""
        for _ in range(10):
            snapshot = self.kernel32.CreateToolhelp32Snapshot(0x8 | 0x10, pid)  # TH32CS_SNAPMODULE(32)
            if snapshot and snapshot != ctypes.c_void_p(-1).value:
                break
            time.sleep(0.1)
        else:
            return None
        try:
            found = []
            entry = ModuleEntry(dwSize=ctypes.sizeof(ModuleEntry))
            more = self.kernel32.Module32FirstW(snapshot, ctypes.byref(entry))
            while more:
                found.append(entry.szExePath)
                more = self.kernel32.Module32NextW(snapshot, ctypes.byref(entry))
            return found
        finally:
            self.kernel32.CloseHandle(snapshot)

    def owner(self, window):
        """(process id, thread id) owning a window; (0, 0) for none."""
        pid = ctypes.c_uint32()
        thread = self.user32.GetWindowThreadProcessId(window, ctypes.byref(pid))
        return pid.value, thread

    def foreground(self):
        return self.user32.GetForegroundWindow() or 0

    def top_level(self, window):
        return (self.user32.GetAncestor(window, GA_ROOT) or 0) == window

    def visible(self, window):
        return bool(self.user32.IsWindow(window) and self.user32.IsWindowVisible(window)
                    and not self.user32.IsIconic(window))

    def lock_timeout(self):
        value = ctypes.c_uint32()
        if not self.user32.SystemParametersInfoW(SPI_GETFOREGROUNDLOCKTIMEOUT, 0, ctypes.byref(value), 0):
            return None
        return value.value

    def window_state(self, window):
        """What Windows reports about a top-level window: shown, minimized,
        its placement (showCmd and normal rectangle) and its rectangle."""
        placement = Placement(length=ctypes.sizeof(Placement))
        rect = Rect()
        state = {"visible": bool(self.user32.IsWindowVisible(window)), "iconic": bool(self.user32.IsIconic(window))}
        if self.user32.GetWindowPlacement(window, ctypes.byref(placement)):
            normal = placement.normal
            state["placement"] = {"showCmd": placement.showCmd,
                                  "normal": [normal.left, normal.top, normal.right, normal.bottom]}
        if self.user32.GetWindowRect(window, ctypes.byref(rect)):
            state["rect"] = [rect.left, rect.top, rect.right, rect.bottom]
        return state

    def z_order(self, named):
        """Where the `named` top-level windows stand in the desktop's Z-order
        (0 = top, walking GetTopWindow/GW_HWNDNEXT), whether each is
        always-on-top, and the visible windows above the lowest of them with
        their owners, so the evidence shows what covers what."""
        positions, above, index = {}, [], 0
        window = self.user32.GetTopWindow(None) or 0
        order = []
        while window and index < Z_ORDER_LIMIT:
            order.append(window)
            window = self.user32.GetWindow(window, GW_HWNDNEXT) or 0
            index += 1
        for name, handle in named.items():
            positions[name] = order.index(handle) if handle in order else None
        found = [position for position in positions.values() if position is not None]
        lowest = max(found) if found else None
        if lowest is not None:
            for position, handle in enumerate(order[:lowest]):
                if self.user32.IsWindowVisible(handle):
                    above.append({"position": position, "window": handle, "owner": self.owner(handle)[0]})
        topmost = {name: bool(self.user32.GetWindowLongPtrW(handle, GWL_EXSTYLE) & WS_EX_TOPMOST)
                   for name, handle in named.items()}
        return {"positions": positions, "topmost": topmost, "visibleAbove": above[:16], "walked": len(order)}

    def class_name(self, window):
        buffer = ctypes.create_unicode_buffer(256)
        return buffer.value if window and self.user32.GetClassNameW(window, buffer, 256) else None

    def thread_focus(self, window):
        """The keyboard focus of the GUI thread that owns `window`, read
        across processes (GetGUIThreadInfo): its active and focus windows,
        the focus window's class (Chrome_RenderWidgetHostHWND is web
        contents; anything else is the browser's own UI) and whether the
        active window is `window`."""
        info = GuiThreadInfo(cbSize=ctypes.sizeof(GuiThreadInfo))
        thread = self.owner(window)[1]
        if not thread or not self.user32.GetGUIThreadInfo(thread, ctypes.byref(info)):
            return {"error": f"GetGUIThreadInfo failed ({ctypes.get_last_error()})"}
        focus, active = info.hwndFocus or 0, info.hwndActive or 0
        return {"hwndFocus": focus, "focusClass": self.class_name(focus), "hwndActive": active,
                "activeIsWindow": active == window,
                "focusInWebContents": self.class_name(focus) == "Chrome_RenderWidgetHostHWND"}

    def foreground_lock(self):
        """Windows' foreground lock: its timeout and the time since the last
        input. While the time since input is below the timeout, a process
        that is not in front may not take the foreground unless it was
        given the right (AllowSetForegroundWindow)."""
        timeout = self.lock_timeout()
        last = LastInput(cbSize=ctypes.sizeof(LastInput))
        idle = None
        if self.user32.GetLastInputInfo(ctypes.byref(last)):
            idle = (self.kernel32.GetTickCount() - last.dwTime) & 0xFFFFFFFF
        return {"timeout": timeout, "idleMs": idle,
                "active": bool(timeout and idle is not None and idle < timeout)}

    def wait_foreground(self, window, bound=FOREGROUND_WAIT):
        deadline = time.monotonic() + bound
        while True:
            if self.foreground() == window:
                return True
            if time.monotonic() >= deadline:
                return False
            time.sleep(0.05)

    def switch_to(self, window, what, change=True):
        """Bring a top-level window to the foreground, as the OS's window
        switching does. Never synthesizes input. With `change`, the window
        must not already be in front (the switch would prove nothing)."""
        before = self.foreground()
        if before == window:
            require(not change, f"{what} is already in front; the switch would prove nothing")
            return {"route": "already", "before": before}
        self.user32.SetForegroundWindow(window)
        if self.wait_foreground(window):
            return {"route": "SetForegroundWindow", "before": before}
        # Windows refused the change (its foreground lock): join the current
        # foreground thread's input state for the call, as task switchers do.
        current = self.foreground()
        thread = self.owner(current)[1] if current else 0
        me = self.kernel32.GetCurrentThreadId()
        attached = bool(thread and thread != me and self.user32.AttachThreadInput(me, thread, True))
        try:
            self.user32.BringWindowToTop(window)
            self.user32.SetForegroundWindow(window)
        finally:
            if attached:
                self.user32.AttachThreadInput(me, thread, False)
        if self.wait_foreground(window):
            return {"route": "AttachThreadInput", "before": before}
        raise Failure(f"{what} did not come to the front (foreground {self.foreground()}, "
                      f"foreground lock timeout {self.lock_timeout()} ms)")

    def minimize(self, window):
        """Minimize another process's top-level window without activating
        another one (ShowWindowAsync, so a busy owner cannot block this
        process). No input event is sent."""
        self.user32.ShowWindowAsync(window, SW_SHOWMINNOACTIVE)

    def iconic(self, window):
        return bool(self.user32.IsIconic(window))

    def close(self, window):
        """A native close request (WM_CLOSE), as the title bar's close button."""
        require(self.user32.PostMessageW(window, WM_CLOSE, 0, 0), f"WM_CLOSE to {window} was refused")

    def listeners(self, port):
        """Owning process ids of IPv4 TCP listeners on 127.0.0.1:`port`."""
        size = ctypes.c_uint32(0)
        result = self.iphlpapi.GetExtendedTcpTable(None, ctypes.byref(size), False, AF_INET,
                                                   TCP_TABLE_OWNER_PID_LISTENER, 0)
        require(result == ERROR_INSUFFICIENT_BUFFER, f"GetExtendedTcpTable size query failed ({result})")
        for _ in range(4):
            buffer = ctypes.create_string_buffer(size.value + 4096)
            size = ctypes.c_uint32(len(buffer))
            result = self.iphlpapi.GetExtendedTcpTable(buffer, ctypes.byref(size), False, AF_INET,
                                                       TCP_TABLE_OWNER_PID_LISTENER, 0)
            if result == 0:
                return loopback_listeners(buffer.raw[:size.value], port)
            require(result == ERROR_INSUFFICIENT_BUFFER, f"GetExtendedTcpTable failed ({result})")
        raise Failure("The TCP table kept growing")


def loopback_listeners(table, port):
    """Parse MIB_TCPTABLE_OWNER_PID: a count, then rows of six DWORDs (state,
    local address, local port, remote address, remote port, owning pid), the
    address in network order and the port in the low 16 bits, network order."""
    (count,) = struct.unpack_from("<I", table, 0)
    require(4 + 24 * count <= len(table), "Truncated TCP table")
    loopback = struct.unpack("<I", socket.inet_aton("127.0.0.1"))[0]
    owners = []
    for index in range(count):
        state, address, local_port, _, _, pid = struct.unpack_from("<6I", table, 4 + 24 * index)
        if state == MIB_TCP_STATE_LISTEN and address == loopback and socket.ntohs(local_port & 0xFFFF) == port:
            owners.append(pid)
    return owners


# --- DevTools ----------------------------------------------------------------

def mask_bytes(payload, mask):
    return bytes(byte ^ mask[index & 3] for index, byte in enumerate(payload))


class WebSocket:
    """A minimal RFC 6455 client: text messages, masked client frames,
    fragmented server messages, ping/pong and close. A frame is taken from the
    buffer only once it is complete, so a read that times out loses nothing."""

    def __init__(self, host, port, path, timeout=5.0, connection=None):
        self.sock = connection or socket.create_connection((host, port), timeout=timeout)
        self.buffer = b""
        self.partial = None
        key = base64.b64encode(os.urandom(16)).decode("ascii")
        self.sock.settimeout(timeout)
        self.sock.sendall((f"GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nUpgrade: websocket\r\n"
                           f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n")
                          .encode("ascii"))
        deadline = time.monotonic() + timeout
        while b"\r\n\r\n" not in self.buffer:
            require(len(self.buffer) < 16384, "Oversized WebSocket handshake")
            self._fill(deadline)
        head, self.buffer = self.buffer.split(b"\r\n\r\n", 1)
        lines = head.decode("latin-1").split("\r\n")
        require(lines[0].split(" ")[1:2] == ["101"], f"WebSocket upgrade refused: {lines[0][:200]!r}")
        headers = {name.strip().lower(): value.strip()
                   for name, value in (line.split(":", 1) for line in lines[1:] if ":" in line)}
        accept = base64.b64encode(hashlib.sha1((key + GUID).encode("ascii")).digest()).decode("ascii")
        require(headers.get("sec-websocket-accept") == accept, "WebSocket accept key mismatch")

    # CDP_RECEIVE_OBSERVATION_BEGIN
    def _observe_receive(self, stage, received=None):
        try:
            state = getattr(self, "last_receive_observation", None)
            if stage == "start":
                self.receive_observation_error = False
            elif state is None:
                return  # Do not attribute the constructor's HTTP handshake to a receive.
            buffer, partial = self.buffer, self.partial
            if type(buffer) is not bytes or (partial is not None and type(partial) is not bytearray):
                raise ValueError("Invalid receive observation")
            if stage == "read" and type(received) is not bytes:
                raise ValueError("Invalid receive observation")
            self.last_receive_observation = receive_observation(
                state, stage, buffered=len(buffer), fragment=0 if partial is None else len(partial),
                fragment_active=partial is not None, size=len(received) if stage == "read" else 0)
        except Exception:
            try:
                self.receive_observation_error = True
            except Exception:
                pass
    # CDP_RECEIVE_OBSERVATION_END
    def _fill(self, deadline):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            self._observe_receive("deadline")  # CDP_RECEIVE_OBSERVATION
            raise TimeoutError("WebSocket read timed out")
        self.sock.settimeout(remaining)
        self._observe_receive("socket")  # CDP_RECEIVE_OBSERVATION
        chunk = self.sock.recv(65536)
        self._observe_receive("read", chunk)  # CDP_RECEIVE_OBSERVATION
        if not chunk:
            self._observe_receive("socket_closed")  # CDP_RECEIVE_OBSERVATION
            raise ConnectionError("WebSocket closed")
        self.buffer += chunk

    def _frame(self, deadline):
        """(fin, opcode, payload) of the next complete frame."""
        while True:
            if len(self.buffer) >= 2:
                first, second = self.buffer[0], self.buffer[1]
                length, offset = second & 0x7F, 2
                if length == 126 and len(self.buffer) >= 4:
                    length, offset = int.from_bytes(self.buffer[2:4], "big"), 4
                elif length == 127 and len(self.buffer) >= 10:
                    length, offset = int.from_bytes(self.buffer[2:10], "big"), 10
                if (second & 0x7F) < 126 or offset > 2:
                    require(length <= MAX_MESSAGE, "Oversized WebSocket frame")
                    masked = second & 0x80
                    end = offset + (4 if masked else 0) + length
                    if len(self.buffer) >= end:
                        mask = self.buffer[offset:offset + 4] if masked else None
                        payload = self.buffer[end - length:end]
                        self.buffer = self.buffer[end:]
                        self._observe_receive("frame")  # CDP_RECEIVE_OBSERVATION
                        return bool(first & 0x80), first & 0x0F, mask_bytes(payload, mask) if mask else payload
            self._observe_receive("frame_wait")  # CDP_RECEIVE_OBSERVATION
            self._fill(deadline)

    def send_text(self, text):
        self._send(0x1, text.encode("utf-8"))

    def _send(self, opcode, payload):
        header = bytearray([0x80 | opcode])
        if len(payload) < 126:
            header.append(0x80 | len(payload))
        elif len(payload) < 65536:
            header.append(0x80 | 126)
            header += len(payload).to_bytes(2, "big")
        else:
            header.append(0x80 | 127)
            header += len(payload).to_bytes(8, "big")
        mask = os.urandom(4)
        self.sock.settimeout(5)
        self.sock.sendall(bytes(header) + mask + mask_bytes(payload, mask))

    def receive(self, deadline):
        """The next text message; TimeoutError when none completes in time."""
        self._observe_receive("start")  # CDP_RECEIVE_OBSERVATION
        while True:
            fin, opcode, payload = self._frame(deadline)
            if opcode == 0x8:
                self._observe_receive("frame_closed")  # CDP_RECEIVE_OBSERVATION
                raise ConnectionError("WebSocket closed by the browser")
            if opcode == 0x9:
                self._send(0xA, payload)
                continue
            if opcode == 0xA:
                continue
            if opcode in (0x1, 0x2):
                require(self.partial is None, "Interleaved WebSocket message")
                self.partial = bytearray(payload)
            elif opcode == 0x0:
                require(self.partial is not None, "Unexpected WebSocket continuation")
                self.partial += payload
            else:
                raise Failure(f"Unexpected WebSocket opcode {opcode}")
            require(len(self.partial) <= MAX_MESSAGE, "Oversized WebSocket message")
            if fin:
                message, self.partial = bytes(self.partial), None
                self._observe_receive("message_bytes")  # CDP_RECEIVE_OBSERVATION
                return message.decode("utf-8")

    def close(self):
        try:
            self._send(0x8, b"")
        except OSError:
            pass
        self.sock.close()


class CdpError(Failure):
    """A DevTools command's error reply; `error` holds its code and message."""

    def __init__(self, text, error):
        super().__init__(text)
        self.error = error


# CDP_RECEIVE_OBSERVATION_BEGIN
_RECEIVE_LIMIT = 2**31 - 1
_RECEIVE_BOUNDARIES = ("start", "frame_wait", "socket", "read", "frame", "message_bytes",
                       "deadline", "socket_closed", "frame_closed")
_RECEIVE_NUMBERS = ("readsReturned", "bytesReceived", "frames", "bufferedBytes", "fragmentBytes")


def receive_observation(state, stage, buffered=None, fragment=None, fragment_active=None, size=0):
    """One receive call's closed progress; buffer/fragment bytes may predate this call."""
    if type(stage) is not str:
        raise ValueError("Invalid receive observation")
    if stage == "start":
        result = {"boundary": "start", "readsReturned": 0, "bytesReceived": 0, "frames": 0,
                  "bufferedBytes": 0, "fragmentBytes": 0, "fragmentActive": False,
                  "countsCapped": False, "closed": False}
    else:
        keys = set(_RECEIVE_NUMBERS) | {"boundary", "fragmentActive", "countsCapped", "closed"}
        if type(state) is not dict or set(state) != keys:
            raise ValueError("Invalid receive observation")
        if type(state["boundary"]) is not str or state["boundary"] not in _RECEIVE_BOUNDARIES:
            raise ValueError("Invalid receive observation")
        if any(type(state[key]) is not int or not 0 <= state[key] <= _RECEIVE_LIMIT for key in _RECEIVE_NUMBERS):
            raise ValueError("Invalid receive observation")
        if any(type(state[key]) is not bool for key in ("fragmentActive", "countsCapped", "closed")):
            raise ValueError("Invalid receive observation")
        if ((not state["fragmentActive"] and state["fragmentBytes"] != 0)
                or state["closed"] != (state["boundary"] in ("socket_closed", "frame_closed"))):
            raise ValueError("Invalid receive observation")
        result = dict(state)
    if stage == "snapshot" and buffered is None and fragment is None and fragment_active is None:
        return result
    if (stage != "snapshot" and stage not in _RECEIVE_BOUNDARIES) or type(fragment_active) is not bool:
        raise ValueError("Invalid receive observation")
    for key, value in (("bufferedBytes", buffered), ("fragmentBytes", fragment)):
        if type(value) is not int or value < 0:
            raise ValueError("Invalid receive observation")
        result["countsCapped"] |= value > _RECEIVE_LIMIT
        result[key] = min(value, _RECEIVE_LIMIT)
    result.update(boundary=result["boundary"] if stage == "snapshot" else stage, fragmentActive=fragment_active)
    if stage == "read":
        if type(size) is not int or size < 0:
            raise ValueError("Invalid receive observation")
        for key, increment in (("readsReturned", 1), ("bytesReceived", size)):
            value = result[key] + increment
            result["countsCapped"] |= value > _RECEIVE_LIMIT
            result[key] = min(value, _RECEIVE_LIMIT)
    if stage == "frame":
        result["countsCapped"] |= result["frames"] == _RECEIVE_LIMIT
        result["frames"] = min(result["frames"] + 1, _RECEIVE_LIMIT)
    result["closed"] |= stage in ("socket_closed", "frame_closed")
    return result


def receive_snapshot(outcome, transport=None, observationError=False):
    if type(outcome) is not str or outcome not in ("message", "timeout", "connection_error", "exception"):
        raise ValueError("Invalid receive observation")
    if type(observationError) is not bool:
        raise ValueError("Invalid receive observation")
    return {"outcome": outcome, "transport": None if observationError or transport is None else
            receive_observation(transport, "snapshot"), "observationError": observationError}
# CDP_RECEIVE_OBSERVATION_END
# CDP_COMMAND_OBSERVATION_BEGIN
# Closed metadata only: never store a method, label, session or message value.
def command_observation(record, stage, **values):
    if stage == "start":
        ident = values["ident"]
        return {"kind": "takosumi.cdp-command-observation@v1", "clockDomain": "host_monotonic",
                "clockSource": "time.monotonic", "clockUnit": "s",
                "commandId": ident if 0 <= ident <= 2**53 - 1 else None,
                "sendReturned": False, "matchingReplies": 0, "otherReplies": 0, "events": 0,
                "countsCapped": False, "channelClosed": False, "phase": "send", "outcome": "sending",
                "lastReceive": None, "lastOtherReplyRelation": None,
                "firstEventReceivedAt": None, "lastEventReceivedAt": None}
    if record is None:
        return None
    result = dict(record)
    if stage == "wait":
        result.update(sendReturned=True, sendReturnedAt=values["sent_at"], deadlineAt=values["deadline"],
                      phase="receive", outcome="waiting")
    elif stage == "message":
        for name, increment in (("matchingReplies", values["matching"]),
                                ("otherReplies", values["other"]), ("events", values["event"])):
            if increment:
                result["countsCapped"] |= result[name] == 2**31 - 1
                result[name] = min(result[name] + 1, 2**31 - 1)
        if values["event"]:
            # Already stamped after JSON classification; no clock read or raw event is retained.
            received_at = values["message"][RECEIVED_AT]
            if type(received_at) is not float or not 0 <= received_at < float("inf"):
                raise ValueError("Invalid event receipt time")
            if result["firstEventReceivedAt"] is None:
                result["firstEventReceivedAt"] = received_at
            result["lastEventReceivedAt"] = received_at
        if values["other"]:
            relation = command_reply_relation(values["reply_id"], values["ident"])
            if type(relation) is not str or relation not in ("older", "future", "invalid", "unknown"):
                raise ValueError("Invalid closed reply relation")
            result["lastOtherReplyRelation"] = relation
    elif stage == "receive":
        result["lastReceive"] = receive_snapshot(**values["snapshot"])
    elif stage == "end":
        result.update(outcome=values["outcome"], phase="reply")
    elif stage == "error":
        result.update(sendReturned=values["sent"], channelClosed=values["closed"],
                      phase=values["phase"], errorCategory=values["category"])
        if result["outcome"] in ("sending", "waiting"):
            result["outcome"] = "exception"
    return result



def command_reply_relation(reply_id, ident):
    """Last nonmatching ID only; not proof of a late reply or a valid remote command.

    Invalid means outside the positive exact-integer diagnostic range, not a
    change to the original Python equality used to accept matching replies.
    """
    if type(reply_id) is not int or not 1 <= reply_id <= 2**53 - 1:
        return "invalid"
    if type(ident) is not int or not 1 <= ident <= 2**53 - 1 or reply_id == ident:
        return "unknown"
    return "older" if reply_id < ident else "future"


def command_error_category(error, closed):
    if closed:
        return "channel_closed"
    if isinstance(error, CdpError):
        return "cdp_error"
    if isinstance(error, TimeoutError):
        return "timeout"
    if isinstance(error, ConnectionError):
        return "connection"
    if isinstance(error, OSError):
        return "os"
    if isinstance(error, json.JSONDecodeError):
        return "json"
    if isinstance(error, Failure):
        return "failure"
    return "other"
# CDP_COMMAND_OBSERVATION_END


class Cdp:
    """One browser's DevTools connection. Events are kept (the oldest dropped
    past 4096) and addressed by absolute marks."""

    def __init__(self, websocket, label):
        self.websocket, self.label = websocket, label
        self.next = 0
        self.events = []
        self.base = 0
        self.closed = None
        # When each mark was taken (monotonic); events carry their receipt
        # time as RECEIVED_AT, for dialog_evidence.
        self.marked_at = {}
        self.last_command_observation = None
        self.command_observation_error = False
        self._last_receive_observation = None  # CDP_RECEIVE_OBSERVATION

    def mark(self):
        mark = self.base + len(self.events)
        self.marked_at[mark] = time.monotonic()
        return mark

    def since(self, mark):
        return self.events[max(0, mark - self.base):]

    # CDP_RECEIVE_OBSERVATION_BEGIN
    def _remember_receive(self, outcome):
        try:
            if isinstance(self.websocket, WebSocket):
                self.websocket._observe_receive("snapshot")
            self._last_receive_observation = receive_snapshot(
                outcome, getattr(self.websocket, "last_receive_observation", None),
                getattr(self.websocket, "receive_observation_error", False))
            if self._last_receive_observation["observationError"]:
                self.command_observation_error = True
        except Exception:
            self._last_receive_observation = {"outcome": outcome, "transport": None, "observationError": True}
            self.command_observation_error = True
    # CDP_RECEIVE_OBSERVATION_END
    def _next(self, deadline):
        try:
            message = json.loads(self.websocket.receive(deadline))
        except TimeoutError:
            self._remember_receive("timeout")  # CDP_RECEIVE_OBSERVATION
            return None
        except (ConnectionError, OSError) as error:
            self._remember_receive("connection_error")  # CDP_RECEIVE_OBSERVATION
            self.closed = str(error)
            raise Failure(f"DevTools connection of {self.label} ended: {error}") from None
        except Exception:  # CDP_RECEIVE_OBSERVATION
            self._remember_receive("exception")  # CDP_RECEIVE_OBSERVATION
            raise  # CDP_RECEIVE_OBSERVATION
        self._remember_receive("message")  # CDP_RECEIVE_OBSERVATION
        if "method" in message:
            message[RECEIVED_AT] = time.monotonic()
            self.events.append(message)
            if len(self.events) > 4096:
                del self.events[:1024]
                self.base += 1024
        return message

    # CDP_COMMAND_OBSERVATION_BEGIN
    def _observe_command(self, stage, **values):
        try:
            if stage == "start":
                self._last_receive_observation = None
                self.last_command_observation = None
                self.command_observation_error = False
            if stage == "error":
                values["category"] = command_error_category(values.pop("error"), values["closed"])
            self.last_command_observation = command_observation(self.last_command_observation, stage, **values)
        except Exception:
            self.command_observation_error = True
    # CDP_COMMAND_OBSERVATION_END

    def command(self, method, params=None, session=None, timeout=5.0):
        self.next += 1
        ident = self.next
        request = {"id": ident, "method": method, "params": params or {}}
        if session:
            request["sessionId"] = session
        self._observe_command("start", ident=ident)
        sent, phase = False, "send"
        try:
            self.websocket.send_text(json.dumps(request))
            sent = True
            sent_at = time.monotonic()
            deadline = sent_at + timeout
            phase = "receive"
            self._observe_command("wait", sent_at=sent_at, deadline=deadline)
            while True:
                message = self._next(deadline)
                self._observe_command("receive", snapshot=self._last_receive_observation)  # CDP_RECEIVE_OBSERVATION
                if message is None:
                    self._observe_command("end", outcome="timeout")
                    raise Failure(f"CDP timeout {method} ({self.label})")
                reply_id = message.get("id")
                if reply_id == ident:
                    self._observe_command("message", matching=True, other=False, event="method" in message, message=message)
                    phase = "reply"
                    if "error" in message:
                        self._observe_command("end", outcome="cdp_error")
                        raise CdpError(f"CDP {method} failed ({self.label}): {json.dumps(message['error'])[:500]}",
                                       message["error"])
                    self._observe_command("end", outcome="success")
                    return message.get("result", {})
                else:
                    self._observe_command("message", matching=False, other="id" in message, event="method" in message,
                                          reply_id=reply_id, ident=ident, message=message)
        except Exception as error:
            if phase == "receive":  # CDP_RECEIVE_OBSERVATION
                self._observe_command("receive", snapshot=self._last_receive_observation)  # CDP_RECEIVE_OBSERVATION
            self._observe_command("error", sent=sent, closed=self.closed is not None, phase=phase, error=error)
            raise

    def wait_event(self, predicate, mark, timeout, what):
        deadline = time.monotonic() + timeout
        while True:
            for event in self.since(mark):
                if predicate(event):
                    return event
            if self._next(deadline) is None:
                raise Failure(what)

    def drain(self, seconds=0.3):
        """Take whatever events arrive within `seconds`."""
        deadline = time.monotonic() + seconds
        while self._next(deadline) is not None:
            pass


def inside(image, directory):
    """Whether `image`, a process's executable path, lies in `directory`
    (Windows paths, compared case-insensitively)."""
    if not image:
        return False
    root = ntpath.normcase(ntpath.normpath(str(directory))).rstrip("\\")
    return ntpath.normcase(ntpath.normpath(image)).startswith(root + "\\")


CRT_MODULES = ("vcruntime140", "msvcp140")
# The sandboxed browser's code (W7), beside CEF's bootstrap in cef\.
BROWSER_DLL = "takosumi-cef-browser-process.dll"


def module_problems(role, modules, cef, code=None):
    """What a W6 process's loaded modules show against the static C runtime
    and the installed bundle: no Visual C++ runtime module in either process,
    and the browser's libcef.dll from the installation's cef directory. With
    `code` (W7: the browser DLL CEF's bootstrap loads, when the installation
    has one), the browser must have loaded that DLL from there as well."""
    if modules is None:
        return [f"no module snapshot of the {role}"]
    problems = [f"the {role} loaded {path}" for path in modules
                if ntpath.basename(path).lower().startswith(CRT_MODULES)]
    if role == "browser":
        for name in ("libcef.dll",) + ((code,) if code else ()):
            found = [path for path in modules if ntpath.basename(path).lower() == name.lower()]
            if not found:
                problems.append(f"the browser has not loaded {name}")
            problems += [f"the browser loaded {name} from {path}" for path in found if not inside(path, cef)]
    return problems


def url_origin(url):
    """The origin part (scheme and authority) of a dialog's URL."""
    return urlsplit(url)._replace(path="", query="", fragment="").geturl()


def final_quit_dialog(params, app_origin):
    """W4: the one dialog the final quit may accept on its own, the real
    site's beforeunload on that site; any other type or origin fails. Returns
    what is accepted."""
    found = {"type": params.get("type"), "origin": url_origin(params.get("url") or "")}
    require(found["type"] == "beforeunload" and found["origin"] == app_origin,
            f"At the final quit only a beforeunload of {app_origin} may be accepted, not {found}")
    return found


# Dialog failure evidence (dialog_evidence): the page session's dialog events
# and its frame lifecycle and navigation events, without text values.
RECEIVED_AT = "_receivedAt"
DIALOG_EVENTS = ("Page.javascriptDialogOpening", "Page.javascriptDialogClosed")
FRAME_EVENTS = ("Page.frameAttached", "Page.frameDetached", "Page.frameStartedLoading", "Page.frameNavigated",
                "Page.frameRequestedNavigation", "Page.navigatedWithinDocument")
FRAME_EVIDENCE_LIMIT = 200


def dialog_evidence(events, session, start):
    """From `events` on `session`: each dialog opening (type, origin, browser
    handler) and closing (result, and whether userInput was absent, empty or
    present), and each frame lifecycle or navigation event (frame and parent
    ids, URL origin; the last FRAME_EVIDENCE_LIMIT and their total), with
    its receipt time in seconds after `start` (the mark's time). No message,
    prompt, user input or full URL is kept."""
    dialogs, frames, total = [], [], 0
    for event in events:
        method = event.get("method")
        if event.get("sessionId") != session or method not in DIALOG_EVENTS + FRAME_EVENTS:
            continue
        params = event.get("params") or {}
        entry = {"event": method.split(".", 1)[1]}
        if start is not None and RECEIVED_AT in event:
            entry["at"] = round(event[RECEIVED_AT] - start, 3)
        if method == "Page.javascriptDialogOpening":
            entry.update(type=params.get("type"), origin=url_origin(params.get("url") or ""),
                         hasBrowserHandler=params.get("hasBrowserHandler"))
            dialogs.append(entry)
            continue
        if method == "Page.javascriptDialogClosed":
            entry["result"] = params.get("result")
            entry["userInput"] = ("absent" if "userInput" not in params
                                  else "present" if params["userInput"] else "empty")
            dialogs.append(entry)
            continue
        frame = params.get("frame") or {}
        entry["frameId"] = frame.get("id") or params.get("frameId")
        parent = frame.get("parentId") or params.get("parentFrameId")
        if parent:
            entry["parentId"] = parent
        url = frame.get("url") or params.get("url")
        if url:
            entry["origin"] = url_origin(url)
        total += 1
        frames.append(entry)
    return {"dialogs": dialogs, "frames": frames[-FRAME_EVIDENCE_LIMIT:], "frameEvents": total}

# The DevTools reply to Page.handleJavaScriptDialog when the page has no
# dialog (any more).
NO_DIALOG = "No dialog is showing"


def browser_closed_dialog(events, opened, session):
    """W4: the page session's own javascriptDialogClosed for the dialog
    `opened` (the first dialog event on that session after its opening), or
    None if none arrived yet. Another dialog opening first is a failure."""
    after = False
    for event in events:
        if event is opened:
            after = True
            continue
        if not after or event.get("sessionId") != session:
            continue
        if event.get("method") == "Page.javascriptDialogOpening":
            raise Failure("Another dialog opened before the first one closed")
        if event.get("method") == "Page.javascriptDialogClosed":
            return event
    require(after, "The dialog's opening is no longer among the kept events")
    return None


def real_site_first_checkpoint(site):
    """W4: the observer's worst case, from the App's record of both browsers
    to its first checkpoint (inspect-ready), in a real-site run: attach and
    sign-in in both profiles (LOAD_WAIT each), each real-site stage in both
    (REAL_SITE_WAIT each), then two window switches, the occlusion watch and
    the minimize (two settles). The App's bound for that checkpoint
    (browser_process.rs `inspect_bound`) covers it with a margin; a unit test
    keeps the two in step."""
    stages = 2 if "landing" in REAL_SITES[site] else 1
    windows = 2 * 2 * FOREGROUND_WAIT + OCCLUSION_WATCH + 2 * SETTLE_WAIT
    return 2 * LOAD_WAIT + 2 * LOAD_WAIT + 2 * stages * REAL_SITE_WAIT + windows


class Page:
    """The one top-level document of a profile's browser, through a flattened
    target session on that browser's own DevTools connection."""

    def __init__(self, cdp, session, process, target):
        self.cdp, self.session, self.process, self.target = cdp, session, process, target
        self.native = process["opened"]
        self.window = self.native["windowId"]
        # The fixture page's context (attach), and in a W4 run the real site's.
        self.context = self.site_context = None
        self.site_entry = None

    def send(self, method, params=None, timeout=5.0):
        return self.cdp.command(method, params, self.session, timeout)

    def evaluate(self, expression, gesture=False, timeout=5.0):
        result = self.send("Runtime.evaluate", {"expression": expression, "returnByValue": True,
                                                "awaitPromise": True, "userGesture": gesture},
                           timeout)
        require(not result.get("exceptionDetails"), json.dumps(result.get("exceptionDetails"))[:1000])
        return result["result"].get("value")

    def pages(self):
        return [target["targetId"] for target in self.cdp.command("Target.getTargets")["targetInfos"]
                if target["type"] == "page"]


# --- Observation ---------------------------------------------------------------

class Observer:
    def __init__(self, data, site, app_pid, phase, win32=None, real_site=None, installed=None,
                 expect_sandbox=True, recovery_images=None, recovery_app_data=None):
        self.data, self.site, self.app_pid, self.phase = data, site, app_pid, phase
        # W7: each profile's browser must show Chromium's sandbox (False:
        # the unsandboxed candidate, recorded only).
        self.expect_sandbox = expect_sandbox
        # W6 (--packaged): the installed App's directory; each guard and
        # browser must run from its cef\ (installed_from).
        self.installed = installed
        self.recovery_images = recovery_images
        self.recovery_app_data = recovery_app_data
        # W4: the app link leads to this real site (None: the fixture's /app/).
        self.real = REAL_SITES[real_site] if real_site else None
        self.real_name = real_site
        self.app_origin = self.real["origin"] if self.real else site
        self.suffix = f"-{phase}" if phase else ""
        self.nonce = data.name
        self.key = f"sumi_app_process_{self.nonce}"
        self.win = win32
        self.clients = []
        self.proof = {"passed": False, "cases": [],
                      "scope": "Disposable Windows actual App/browser process fixture; local fixture site, "
                               "no real login or Push"}
        if self.real:
            self.proof["scope"] = (f"Disposable Windows actual App/browser process fixture; local fixture home, "
                                   f"anonymous {real_site} app, synthetic markers only, no real login")

    # Helpers shared with browser-process.mjs's shape.
    def step(self, name, page=None):
        self.proof["stage"] = {"name": name, "pid": page.native["nativePid"] if page else None,
                               "profile": page.process["profile"] if page else None}

    def case(self, name, evidence=None):
        self.proof["cases"].append(name)
        if evidence is not None:
            self.proof.setdefault("evidence", {})[name] = evidence

    def json_file(self, name):
        with open(self.data / name, "rb") as source:
            return json.loads(source.read(4 * 1024 * 1024))

    def marker(self, name):
        deadline = time.monotonic() + MARKER_WAIT
        while time.monotonic() < deadline:
            # Markers are written non-atomically: an incomplete file is not yet
            # ready (nor, on Windows, one its writer still holds exclusively).
            try:
                return self.json_file(name)
            except (FileNotFoundError, PermissionError, ValueError):
                time.sleep(0.05)
        raise Failure(f"Missing App checkpoint: {name}")

    def checkpoint(self, name):
        with open(self.data / name, "x", encoding="utf-8") as output:
            output.write("ready")

    @staticmethod
    def settled(test, message, bound=SETTLE_WAIT, tolerate=False):
        """Until `test()` holds. With `tolerate`, a failing evaluation (a page
        between documents) counts as not yet."""
        deadline = time.monotonic() + bound
        while time.monotonic() < deadline:
            try:
                if test():
                    return
            except Failure:
                if not tolerate:
                    raise
            time.sleep(0.05)
        raise Failure(message)

    @staticmethod
    def guard_ended(receipt):
        # The guard creates its receipt when it starts and writes JSON only at its end.
        try:
            with open(receipt, "rb") as source:
                text = source.read()
        except (FileNotFoundError, PermissionError):
            return False
        if not text:
            return False
        try:
            json.loads(text)
            return True
        except ValueError:
            return False

    def normal_receipt(self, receipt, message, bound=SETTLE_WAIT):
        self.settled(lambda: self.guard_ended(receipt), message, bound)
        with open(receipt, "rb") as source:
            value = json.loads(source.read())
        self.check_normal_receipt(value, message)
        return value

    @staticmethod
    def check_normal_receipt(value, message):
        require(value["rootExit"] == 0 and value["complete"] is True and value["kernelNoChildren"] is True
                and value["forcedCleanup"] == 0 and value["remaining"] == [] and value["cleanupErrors"] == []
                and value["error"] is None, f"{message}: abnormal receipt {json.dumps(value)[:500]}")

    def cookie_of(self):
        key = self.key
        return (f"(document.cookie.split('; ').find(c=>c.startsWith({json.dumps(key + '=')}))"
                f"?.slice({len(key) + 1})??null)")

    def sample(self, page):
        key = json.dumps(self.key)
        return self.read_evaluate(page,
            f"({{document:globalThis[{key}],storage:localStorage.getItem({key}),cookie:{self.cookie_of()},"
            "origin:location.origin,frame:window===top,page:document.querySelector('main')?.dataset.page??null,"
            "user:document.querySelector('main')?.dataset.user??null})")

    def read_evaluate(self, page, expression, gesture=False):
        """One pure read or same-effect write through a starved DevTools
        channel. A busy renderer can starve one reply while the cancelled
        quit's churn continues (run 42961067390: Photopea kept Work's
        renderer busy through the post-cancel sample; run 42983025866:
        Photopea starved even the reply to the beforeunload listener's own
        removal). A timed-out evaluate still holds its queue position in
        the renderer — run 43061069602 starved ELEVEN re-sent evaluates at
        the back of a still-deep queue while the page's own tasks kept
        running — so the bound is spent waiting on each request already
        sent, not re-queuing behind the churn. Anything else raises."""
        deadline = time.monotonic() + STARVED_READ_WAIT
        attempts = 0
        while True:
            try:
                value = page.evaluate(expression, gesture,
                                      max(5.0, deadline - time.monotonic()))
                break
            except Failure as error:
                if "CDP timeout" in str(error):
                    attempts += 1
                    self.proof.setdefault("starvedEvaluateRetries", []).append(
                        {"attempt": attempts, "error": str(error)[:200]})
                    if time.monotonic() < deadline:
                        time.sleep(0.05)
                        continue
                raise
        return value

    def contended_sample(self, page, mark):
        """A post-quit state read that never waits on the renderer's task
        queue. A sustained ad-helper storm can starve a Runtime.evaluate
        through its whole claimed-position bound (run 43068513364: the
        Personal profile's single claimed position starved all 60 s while
        ~20 renderer helpers kept spawning on the real site; run
        43061069602 starved ELEVEN re-sent evaluates on Work). Every
        element here is serviced by the browser process instead: the
        frame tree's root frame gives the live document's origin, the
        storage service answers localStorage, the network service answers
        cookies, and the already-recorded event stream proves no
        main-frame navigation committed or began and no renderer crash
        landed since the given mark — together the same oracle the
        evaluate sample proves: this document and its synthetic state are
        unchanged (a killed-and-never-reloaded renderer would pass the
        browser-serviced reads, so targetCrashed is part of the scan). A
        frameRequestedNavigation alone is not a commit and
        navigatedWithinDocument keeps the document, so neither counts; a
        same-document fragment load on the root is refused rather than
        risk a load in flight. An engine without the storage read
        domains falls back to the claimed-position renderer read; the
        oracle is unchanged."""
        # Browser-serviced reads still ride the same DevTools channel: under
        # the same storm that dispatched and answered every control-pipe
        # command, run 43090171027 starved Page.getFrameTree's flat
        # SETTLE_WAIT on Work. Each send here claims its queue position and
        # waits on it against ONE shared storm bound; a timeout is recorded
        # and re-sent only while the bound lasts, then raised on its own
        # name — never masked by the storage-domain fallback below.
        deadline = time.monotonic() + STARVED_READ_WAIT
        hard_deadline = time.monotonic() + STARVED_MAX_WAIT
        def starved(method, params=None):
            nonlocal deadline
            while True:
                try:
                    return page.send(method, params,
                                     max(5.0, deadline - time.monotonic()))
                except Failure as error:
                    if "CDP timeout" not in str(error):
                        raise
                    now = time.monotonic()
                    events = getattr(page.cdp, "events", ()) or ()
                    quiet_for = now - events[-1].get(RECEIVED_AT, 0.0) if events else None
                    storm_live = quiet_for is not None and quiet_for < STARVED_QUIET_WAIT
                    self.proof.setdefault("starvedBrowserReadRetries", []).append(
                        {"profile": page.process["profile"], "method": method,
                         "error": str(error)[:200],
                         "channelQuietSeconds": round(quiet_for, 1) if quiet_for is not None else None})
                    if storm_live and now < hard_deadline:
                        deadline = min(now + STARVED_QUIET_WAIT, hard_deadline)
                        continue
                    if now < deadline:
                        time.sleep(0.05)
                        continue
                    raise
        # getFrameTree itself has no storage-domain fallback: without the root
        # frame's id the navigation scan below cannot run at all, so a missing
        # Page domain or a destroyed target fails here on its own name.
        tree = starved("Page.getFrameTree")["frameTree"]["frame"]
        root = tree["id"]
        origin = tree.get("securityOrigin")
        if origin is None:
            split = urlsplit(tree.get("url", ""))
            origin = f"{split.scheme}://{split.netloc}"
        try:
            starved("DOMStorage.enable")
            items = dict(starved("DOMStorage.getDOMStorageItems",
                                 {"storageId": {"securityOrigin": origin, "isLocalStorage": True}})
                         .get("entries") or [])
            cookies = starved("Network.getCookies", {"urls": [origin]})["cookies"]
            state = {"storage": items.get(self.key),
                     "cookie": next((item["value"] for item in cookies if item["name"] == self.key), None),
                     "origin": origin}
        except CdpError as error:
            self.proof.setdefault("browserSampleFallbacks", []).append(
                {"profile": page.process["profile"], "error": str(error)[:200]})
            state = self.sample(page)
        require(page.cdp.base <= mark,
                "Events covering the quit window were dropped; document continuity is unproven")
        # A crashed renderer that never reloads still answers every
        # browser-serviced read above while its document is gone — targetCrashed
        # is session-scoped and emitted without Inspector.enable, so the stream
        # already holds it. A same-document fragment change can fire
        # frameStartedLoading on the root: refused rather than risk a load in
        # flight, and hash churn is not part of the quit window's contract.
        nav = next((event for event in page.cdp.since(mark)
                    if event.get("sessionId") == page.session
                    and (event["method"] == "Inspector.targetCrashed"
                         or event["method"] == "Page.frameNavigated"
                         and (event.get("params") or {}).get("frame", {}).get("id") == root
                         or event["method"] == "Page.frameStartedLoading"
                         and (event.get("params") or {}).get("frameId") == root)), None)
        require(nav is None,
                f"The quit window replaced, began replacing, or crashed the main document: "
                f"{json.dumps(nav)[:300]}")
        return state

    def persisted(self, page):
        key = json.dumps(self.key)
        return self.read_evaluate(page,
            f"({{storage:localStorage.getItem({key}),cookie:{self.cookie_of()},"
            "page:document.querySelector('main')?.dataset.page??null,"
            "user:document.querySelector('main')?.dataset.user??null})")

    def process_of(self, opened, name):
        connection = next((item for item in opened["connections"] if item["name"] == name), None)
        require(connection is not None, f"Missing saved {name}")
        process = next((item for item in opened["processes"] if item["profile"] == connection["profile"]), None)
        require(process is not None, f"No browser process for {name}")
        return process

    def installed_from(self, process):
        """W6: this profile's guard and browser run from the installed App's
        cef directory (its own bundle, resolved without overrides)."""
        cef = ntpath.join(str(self.installed), "cef")
        pids = {"guard": process["pid"], "browser": process["opened"]["nativePid"]}
        images = {role: self.win.image(pid) for role, pid in pids.items()}
        # Their loaded modules: the static C runtime (no vcruntime140 or
        # msvcp140) and the installed libcef.dll, at run time.
        modules = {role: self.win.modules(pid) for role, pid in pids.items()}
        # W7: a sandboxed installation's browser is CEF's bootstrap, which
        # must have loaded the browser DLL beside it.
        code = BROWSER_DLL if Path(cef, BROWSER_DLL).is_file() else None
        problems = [problem for role in pids for problem in module_problems(role, modules[role], cef, code)]
        libcef = [path for path in modules["browser"] or [] if ntpath.basename(path).lower() == "libcef.dll"]
        self.proof.setdefault("installedImages", []).append(dict(
            images, profile=process["profile"], libcef=libcef,
            modules={role: len(found) if found is not None else None for role, found in modules.items()}))
        for role, image in images.items():
            require(inside(image, cef), f"The {role} of {process['profile']} runs from {image}, not from {cef}")
        require(not problems, f"The installed processes of {process['profile']}: {problems}")

    def sandbox(self, process, stage="attach"):
        """W7: this profile's browser and the renderers below it in its
        process tree show Chromium's sandbox (windows_smoke's
        sandbox_evidence); recorded in the observation with its `stage` and,
        unless the browser is the unsandboxed candidate, required."""
        pid = process["opened"]["nativePid"]
        found = sandbox_evidence(ProcessTree(self.win.kernel32, pid), pid, time.monotonic() + SANDBOX_WAIT)
        self.proof.setdefault("sandbox", []).append(dict(found, profile=process["profile"], stage=stage,
                                                         required=self.expect_sandbox))
        if self.expect_sandbox:
            require(not found["problems"],
                    f"The browser of {process['profile']} does not show Chromium's sandbox ({stage}): "
                    f"{found['problems']}")
        return found

    def attach(self, process):
        native = process["opened"]
        self.proof["stage"] = {"name": "attach", "pid": native["nativePid"], "profile": process["profile"]}
        if self.installed:
            self.installed_from(process)
        # Only a browser built with the cef-candidate `fixture-inspection` feature writes it.
        try:
            inspection = json.loads((Path(process["directory"]) / "inspection.json").read_bytes())
        except OSError as error:
            raise Failure(f"No inspection.json: build the browser with --features fixture-inspection ({error})")
        require(inspection["pid"] == native["nativePid"], "inspection.json names another process")
        port = inspection["port"]
        owners = self.win.listeners(port)
        require(owners == [native["nativePid"]],
                f"Only the attested native browser may listen on 127.0.0.1:{port} (listeners {owners})")
        connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
        try:
            connection.request("GET", "/json/version")
            info = json.loads(connection.getresponse().read(65536))
        finally:
            connection.close()
        endpoint = urlsplit(info["webSocketDebuggerUrl"])
        require(endpoint.hostname == "127.0.0.1" and endpoint.port == port, "Unexpected DevTools endpoint")
        cdp = Cdp(WebSocket("127.0.0.1", port, endpoint.path), process["profile"])
        self.clients.append(cdp)
        targets = [target for target in cdp.command("Target.getTargets")["targetInfos"] if target["type"] == "page"]
        require(len(targets) == 1, "Exactly one initial document per process")
        session = cdp.command("Target.attachToTarget", {"targetId": targets[0]["targetId"], "flatten": True})
        page = Page(cdp, session["sessionId"], process, targets[0])
        page.send("Page.enable")
        try:
            self.settled(lambda: page.evaluate(
                f"location.origin==={json.dumps(self.site)} && document.readyState==='complete' "
                "&& Boolean(document.querySelector('main[data-page]'))"),
                "The local fixture page did not load", LOAD_WAIT, tolerate=True)
        except Failure:
            try:
                self.proof["startupDocument"] = page.evaluate(
                    "({url:location.href,ready:document.readyState,title:document.title,"
                    "body:document.body?.innerText.slice(0,500)})")
            except Failure as error:
                self.proof["startupDocument"] = {"error": str(error)}
            raise
        # The page has loaded, so a renderer runs it.
        self.sandbox(process)
        window = native["windowId"]
        owner = self.win.owner(window)[0]
        require(owner == native["nativePid"], f"The browser window {window} belongs to {owner}")
        require(self.win.top_level(window) and self.win.visible(window),
                f"The browser window {window} is not a visible top-level window")
        context = page.evaluate(
            "({origin:location.origin,secure:isSecureContext,tauri:typeof __TAURI_INTERNALS__,"
            "ipc:typeof window.ipc,wry:typeof window.webkit?.messageHandlers?.ipc,"
            "webview2:typeof window.chrome?.webview})")
        require(context["secure"] is True, "The fixture page must be a secure context")
        for field in ("tauri", "ipc", "wry", "webview2"):
            require(context[field] == "undefined", f"A CEF page must not see the App's {field} bridge")
        page.context = context
        self.visibility(page, "attach")
        return page

    def sign_in(self, page, user, open_app=True):
        """The sign-in stand-in, then (with `open_app`) the app home's app, in
        this profile."""
        self.step(f"{user} sign-in stand-in", page)
        require(page.evaluate("document.querySelector('main')?.dataset.page") == "login",
                f"A new {user} profile must start signed out")
        page.evaluate("(()=>{document.querySelector('input[name=user]').value=" + json.dumps(user) +
                      ";document.querySelector('form').requestSubmit();return true})()", True)
        signed = json.dumps(user)
        self.settled(lambda: page.evaluate(
            "document.readyState==='complete' && document.querySelector('main')?.dataset.page==='home' "
            f"&& document.querySelector('main')?.dataset.user==={signed}"),
            f"The {user} sign-in did not reach its app home", LOAD_WAIT, tolerate=True)
        if not open_app:
            return
        self.step(f"{user} opens the app", page)
        page.evaluate("(()=>{document.querySelector('a[data-app=editor]').click();return true})()", True)
        self.settled(lambda: page.evaluate(
            "document.readyState==='complete' && location.pathname==='/app/' "
            "&& document.querySelector('main')?.dataset.page==='app' "
            f"&& document.querySelector('main')?.dataset.user==={signed}"),
            f"The {user} app did not open", LOAD_WAIT, tolerate=True)
        require(page.pages() == [page.target["targetId"]], f"The {user} app must stay in its one window")

    def site_settled(self, page, test, message):
        """Until the real site's `test` holds in `page` (REAL_SITE_WAIT); a
        failure records the document the page was on."""
        try:
            self.settled(lambda: page.evaluate(test), message, REAL_SITE_WAIT, tolerate=True)
        except Failure:
            try:
                # The origin only: a real site's URL can carry keys (e.g.
                # Excalidraw's #room= and #json= fragments).
                self.proof["siteDocument"] = dict(page.evaluate(
                    "({origin:location.origin,ready:document.readyState,title:document.title,"
                    "errorSplash:Boolean(document.querySelector('.ErrorSplash')),"
                    "body:document.body?.innerText.slice(0,500)})"), profile=page.process["profile"])
            except Failure as error:
                self.proof["siteDocument"] = {"profile": page.process["profile"], "error": str(error)}
            raise

    def open_real_site(self, pages):
        """W4: from each signed-in app home, follow the app link to the real
        site in the same window (both start before either is awaited), enter
        the editor from the site's landing page if it shows one, then require
        the editor's readiness, its origin in a secure context without an App
        bridge, and still one page per profile."""
        on_site = f"location.origin==={json.dumps(self.app_origin)} && document.readyState==='complete'"
        for page in pages:
            self.step(f"{page.process['profile']} opens {self.real_name}", page)
            href = page.evaluate("document.querySelector('a[data-app=editor]')?.href ?? null")
            require(href == self.real["url"], f"The app home must link to {self.real['url']}, not {href}")
            page.evaluate("(()=>{document.querySelector('a[data-app=editor]').click();return true})()", True)
        landing = self.real.get("landing")
        for page in pages:
            # "landing": entered with the landing's start button, which every
            # profile here must take: this runs once, on the first launch's
            # fresh profiles (no restart or reset phase, and the reopen reuses
            # the document), so an editor without its landing fails. "direct":
            # a site without a landing (Excalidraw).
            page.site_entry = "direct"
            if not landing:
                continue
            self.step(f"{self.real_name} landing or editor", page)
            self.site_settled(page, f"{on_site} && ({self.real['ready']} || {landing})",
                              f"{self.real_name} did not load")
            require(page.evaluate(landing) and not page.evaluate(self.real["ready"]),
                    f"A fresh profile must show {self.real_name}'s landing before its editor")
            page.evaluate(self.real["enter"], True)
            page.site_entry = "landing"
        for page in pages:
            self.step(f"{self.real_name} ready", page)
            self.site_settled(page, f"{on_site} && {self.real['ready']}", f"{self.real_name} did not become ready")
            context = page.evaluate(
                "({origin:location.origin,secure:isSecureContext,tauri:typeof __TAURI_INTERNALS__,"
                "ipc:typeof window.ipc,wry:typeof window.webkit?.messageHandlers?.ipc,"
                "webview2:typeof window.chrome?.webview})")
            require(context["origin"] == self.app_origin and context["secure"] is True,
                    f"{self.real_name} must load in a secure context: {context}")
            for field in ("tauri", "ipc", "wry", "webview2"):
                require(context[field] == "undefined", f"{self.real_name} must not see the App's {field} bridge")
            require(page.pages() == [page.target["targetId"]], f"{self.real_name} must stay in its one window")
            page.site_context = dict(context, entry=page.site_entry)
        # W7: the real site's own renderers (site isolation, ad frames) are
        # sandboxed too, in each profile.
        for page in pages:
            self.step(f"{self.real_name} sandbox", page)
            self.sandbox(page.process, "real-site")

    def finish_final_quit(self, pages):
        """W4: after the final quit request, a real site may ask its own
        beforeunload (the quit then waits for an answer). Accept only that
        (final_quit_dialog; recorded, as is a refused one) until every
        remaining profile's guard has ended normally. Forced descendant
        cleanup ends a guard too, but the App refuses that quit."""
        accepted = self.proof["finalQuitDialogsAccepted"] = []
        receipts = self.proof["finalQuitReceipts"] = []
        marks = {id(page): page.cdp.mark() for page in pages}
        deadline = time.monotonic() + FINAL_QUIT_WAIT
        remaining = list(pages)
        while remaining:
            require(time.monotonic() < deadline, "The final quit did not end every remaining profile")
            for page in list(remaining):
                if self.guard_ended(page.process["guardReceipt"]):
                    with open(page.process["guardReceipt"], "rb") as source:
                        value = json.loads(source.read())
                    receipts.append({"profile": page.process["profile"], "receipt": value})
                    # Preserve the exact refusal before failing: otherwise
                    # M3 marks this observer passed, then only reports the
                    # outer App timeout because the App retained the profile.
                    self.check_normal_receipt(value, f"Final quit for {page.process['profile']}")
                    remaining.remove(page)
                    continue
                try:
                    page.cdp.drain(0.2)
                except Failure:
                    continue  # its connection closed: the browser is ending
                for event in page.cdp.since(marks[id(page)]):
                    if event["method"] == "Page.javascriptDialogOpening" and event.get("sessionId") == page.session:
                        try:
                            dialog = dict(final_quit_dialog(event["params"], self.app_origin),
                                          profile=page.process["profile"])
                        except Failure:
                            self.proof["finalQuitDialogRefused"] = {
                                "profile": page.process["profile"], "type": event["params"].get("type"),
                                "origin": url_origin(event["params"].get("url") or "")}
                            raise
                        try:
                            page.send("Page.handleJavaScriptDialog", {"accept": True})
                        except Failure:
                            continue  # already closed, or the browser is ending
                        accepted.append(dialog)
                marks[id(page)] = page.cdp.mark()
            time.sleep(0.1)

    def visibility(self, page, when):
        """The page's own visibility and focus and Windows' view of its
        window, recorded under `when` (evidence for reuse-raises-window)."""
        try:
            document = page.evaluate("({visibility:document.visibilityState,focus:document.hasFocus()})")
        except Failure as error:
            document = {"error": str(error)}
        entry = dict(document, profile=page.process["profile"], window=self.win.window_state(page.window))
        self.proof.setdefault("visibility", {}).setdefault(when, []).append(entry)
        return entry

    def raise_refusals(self, page):
        try:
            with open(Path(page.process["directory"]) / "browser.log", "rb") as source:
                text = source.read().decode("utf-8", "replace")
        except FileNotFoundError:
            return []
        return [line[:300] for line in text.splitlines() if RAISE_REFUSED in line]

    def answer_permission(self, page):
        """The App-held prompt round trip. The page asks for a notification
        permission under user activation; this opted-in browser holds the
        runtime callback open and reports it on the pipe instead of showing
        its own prompt. The permission-requested checkpoint hands the prompt
        to the App driver, whose click on the home's own row answers it; the
        page's resolved value is the evidence, never assumed."""
        self.step("notification permission answered through the App", page)
        key = json.dumps("__sumiPermissionProbe")
        state = page.evaluate(
            f"(()=>{{const g=globalThis[{key}]={{result:null,error:null}};"
            f"g.promise=Notification.requestPermission().then(r=>g.result=r,e=>g.error=String(e));"
            f"return {{permission:Notification.permission}};}})()", True)
        require(state["permission"] == "default",
                f"A held prompt needs an undecided permission: {state}")
        self.checkpoint("permission-requested")
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            # A busy renderer can starve one poll's reply (run 42983025866's
            # family); this loop already re-asks within its own 30 s bound,
            # so a starved read just means ask again rather than fail the
            # whole step on the first starved reply.
            try:
                state = page.evaluate(
                    f"(()=>({{permission:Notification.permission,result:globalThis[{key}].result,"
                    f"error:globalThis[{key}].error}}))()", True)
            except Failure as error:
                if "CDP timeout" not in str(error) or time.monotonic() >= deadline:
                    raise
                continue
            if state["result"] or state["error"]:
                break
            time.sleep(0.2)
        require(state.get("result") == "granted",
                f"The App's answer did not reach the held callback: {state}")
        entry = dict(state, profile=page.process["profile"])
        self.proof["permissionAnswered"] = entry
        self.case("permission-answered-by-app", entry)
        return entry

    def media_dismissal(self, page):
        """A held media request has no runtime dismissal of its own: it
        retires with its document. The page raises a real getUserMedia; the
        driver confirms the App's held row and records its label, then the
        page reloads — the row's disappearance (asserted by the driver) is
        the retirement evidence, never the request's own promise. Runs only
        where the driver also opted in via the shared flag."""
        if os.environ.get("TAKOSUMI_BROWSER_PROCESS_MEDIA_DISMISSAL") != "1":
            return
        self.step("media permission held for the App then retired with its document", page)
        key = json.dumps("__sumiMediaProbe")
        state = page.evaluate(
            f"(()=>{{const g=globalThis[{key}]={{stream:null,error:null,settled:false}};"
            f"g.promise=navigator.mediaDevices.getUserMedia({{audio:true}})"
            f".then(s=>{{g.stream=s;g.settled=true;return s}},e=>{{g.error=String(e);g.settled=true}});"
            f"return {{devices:typeof navigator.mediaDevices!=='undefined',secure:isSecureContext}};}})()", True)
        require(state and state["devices"] is True,
                f"A held media request needs navigator.mediaDevices: {state}")
        self.checkpoint("media-requested")
        held = self.marker("media-held-seen.json")
        page.evaluate("(()=>{location.reload();return true})()", True)
        self.checkpoint("media-navigated")
        # The reload replaces the document; let it arrive before the next
        # evaluation (a page between documents fails Runtime.evaluate).
        self.settled(lambda: page.evaluate("document.readyState") == "complete",
                     "The reloaded document did not come back", MARKER_WAIT, tolerate=True)
        self.proof["mediaRetired"] = {"requested": True, "secure": state.get("secure"),
                                      "heldLabel": held.get("label")}

    def dialog(self, page, mark, kind, what, bound=SETTLE_WAIT):
        return page.cdp.wait_event(
            lambda event: event["method"] == "Page.javascriptDialogOpening" and event.get("sessionId") == page.session
            and event["params"].get("type") == kind, mark, bound, what)

    def cancel_dialog(self, page, mark, what, opened=None, others=()):
        """Cancel the page's open dialog through DevTools and see it closed
        with result false (which the browser's own close observer reports):
        returns "observer". On a real site (W4) the page or browser may cancel
        the dialog first (mechanism unknown; run 42334571636): DevTools then
        answers that no dialog is showing, and that counts as the cancel only
        if the page's own javascriptDialogClosed for `opened` has result false
        and the profile's guard has not ended: returns "browser". The
        local-fixture run requires the observer's cancel. A failure records
        the evidence of `page` and of each (page, mark) in `others`
        (dialog_failure) before it goes on."""
        attempt = time.monotonic()
        try:
            send_deadline = attempt + STARVED_SEND_WAIT
            send_retry = 0
            while True:
                try:
                    page.send("Page.handleJavaScriptDialog", {"accept": False})
                    break
                except CdpError as error:
                    no_dialog = error.error.get("message") == NO_DIALOG
                    if self.real and opened is not None and no_dialog:
                        return self.cancelled_by_browser(page, what, opened, attempt, error)
                    # Only after a starved send may "no dialog" mean that send
                    # landed late; before any send it means the browser closed
                    # the dialog itself, which the fixture run must refuse.
                    if no_dialog and send_retry and self.dialog_cancelled_close(page, mark) is not None:
                        return "observer"
                    raise
                except Failure as error:
                    # A busy renderer can starve one DevTools reply while the
                    # dialog stays open (run 42953633716: Photopea kept
                    # spawning renderers through Work's whole cancel window;
                    # run 42971293681 exhausted all three blind sends). A
                    # timed-out send may still land: the dialog's own close,
                    # not another blind send, decides what happened — and a
                    # dead profile cannot be answered at all.
                    if "CDP timeout" not in str(error) or time.monotonic() >= send_deadline:
                        raise
                    send_retry += 1
                    self.proof.setdefault("dialogSendRetries", []).append(
                        {"attempt": send_retry, "error": str(error)[:200]})
                    page.cdp.drain(0.5)
                    if self.dialog_cancelled_close(page, mark) is not None:
                        # The timed-out send landed: on a real site record
                        # through the browser-cancel path (which send or actor
                        # delivered it is unknowable); on the fixture it is
                        # the observer's own cancel.
                        if self.real and opened is not None:
                            return self.cancelled_by_browser(page, what, opened, attempt, error)
                        return "observer"
                    require(not self.guard_ended(page.process["guardReceipt"]),
                            f"{what}: the profile ended while its cancel starved")
            page.cdp.wait_event(
                lambda event: event["method"] == "Page.javascriptDialogClosed"
                and event.get("sessionId") == page.session and event["params"].get("result") is False,
                mark, SETTLE_WAIT, what)
            return "observer"
        except Failure as error:
            self.dialog_failure(what, [(page, mark), *others], error, opened, attempt)
            raise

    def cancelled_by_browser(self, page, what, opened, attempt, error):
        """W4, after DevTools found no dialog to cancel: the page's own close
        of `opened` (waited for up to SETTLE_WAIT) must be a cancel (result
        false) while the profile's guard is still running; recorded under
        dialogsClosedByBrowser."""
        deadline = time.monotonic() + SETTLE_WAIT
        closed = browser_closed_dialog(page.cdp.since(page.cdp.base), opened, page.session)
        while closed is None:
            require(time.monotonic() < deadline, f"{what}: {error}, and the dialog's close was not observed")
            page.cdp.drain(0.1)
            closed = browser_closed_dialog(page.cdp.since(page.cdp.base), opened, page.session)
        require(closed["params"].get("result") is False,
                f"{what}: the dialog was closed with result {closed['params'].get('result')!r}, not cancelled")
        require(not self.guard_ended(page.process["guardReceipt"]),
                f"{what}: the profile's guard ended although its dialog was cancelled")
        self.proof.setdefault("dialogsClosedByBrowser", []).append({
            "what": what, "profile": page.process["profile"], "error": str(error)[:300],
            "closedAfterOpening": round(closed[RECEIVED_AT] - opened[RECEIVED_AT], 3)
            if RECEIVED_AT in closed and RECEIVED_AT in opened else None,
            "cancelAfterOpening": round(attempt - opened[RECEIVED_AT], 3) if RECEIVED_AT in opened else None})
        return "browser"

    def dialog_cancelled_close(self, page, mark):
        """The page session's javascriptDialogClosed with result false among
        the events since `mark` — the only proof a held dialog was cancelled,
        whichever send or actor delivered it."""
        return next((event for event in page.cdp.since(mark)
                     if event["method"] == "Page.javascriptDialogClosed"
                     and event.get("sessionId") == page.session
                     and event["params"].get("result") is False), None)

    def dialog_failure(self, what, watched, error, opened=None, attempt=None):
        """Diagnostics only, after a failed dialog step: for each watched
        (page, mark), its dialog and frame events since the mark
        (dialog_evidence), whether its DevTools connection and target are
        still there, and whether its guard ended (did it leave?) with the
        receipt; and the time from the dialog's opening to the cancel
        attempt. Never answers a dialog."""
        evidence = {"what": what, "error": str(error)[:500], "pages": []}
        if opened is not None and attempt is not None and RECEIVED_AT in opened:
            evidence["cancelAfterOpening"] = round(attempt - opened[RECEIVED_AT], 3)
        self.proof.setdefault("dialogFailures", []).append(evidence)
        for page, mark in watched:
            entry = {"profile": page.process["profile"]}
            evidence["pages"].append(entry)
            try:
                try:
                    page.cdp.drain(0.2)
                except Failure:
                    pass  # its connection closed; page.cdp.closed says how
                entry.update(dialog_evidence(page.cdp.since(mark), page.session, page.cdp.marked_at.get(mark)),
                             mainFrame=page.target["targetId"], connectionClosed=page.cdp.closed)
                if page.cdp.closed is None:
                    try:
                        entry["targets"] = page.pages()
                    except Failure as failure:
                        entry["targets"] = {"error": str(failure)[:300]}
                receipt = page.process["guardReceipt"]
                entry["guardEnded"] = self.guard_ended(receipt)
                if entry["guardEnded"]:
                    with open(receipt, "rb") as source:
                        entry["guardReceipt"] = json.loads(source.read())
            except Exception as failure:  # diagnostics only: keep the original failure
                entry["evidenceError"] = repr(failure)[:300]

    def popup_loaded(self, evaluate, url):
        """Same load bound as the cef-candidate and Linux popup probes: the
        requested fixture page itself, not the new window's initial
        about:blank. The navigation entry keeps the URL actually loaded."""
        deadline = time.monotonic() + LOAD_WAIT
        loaded = False
        while time.monotonic() < deadline and not loaded:
            try:
                loaded = bool(evaluate(
                    f"performance.getEntriesByType('navigation')[0]?.name==={json.dumps(url)}"
                    "&&document.readyState==='complete'"
                    "&&Boolean(document.querySelector('main[data-page]'))"))
            except Failure:
                # A page between documents counts as not yet.
                loaded = False
            if not loaded:
                time.sleep(0.1)
        require(loaded, "Popup document must load the requested fixture page")

    def work_popup_probe(self, work, personal, expected):
        """A sign-in style popup on the local fixture: window.open from Work
        opens a native popup inside Work's own browser process (same
        profile). It shares Work's storage and cookie, reports Work as its
        opener, and hands a result back by postMessage before closing
        itself; Personal sees neither. Synthetic state, not provider login.
        Mirrors the Linux runner's popupProbe."""
        state = {"method": "Standard window.open/postMessage/window.close on the local fixture site; "
                           "synthetic state, not provider login"}
        ref = f"sumi_popup_{self.nonce.replace('-', '')}"
        url = f"{self.site}/?{ref}"
        result = f"popup:{self.nonce}"
        key = json.dumps(self.key)

        def page_targets(page):
            return [target for target in page.cdp.command("Target.getTargets")["targetInfos"]
                    if target["type"] == "page"]

        require([target["targetId"] for target in page_targets(work)] == [work.target["targetId"]],
                "Work must have one document before its popup")
        state["accepted"] = work.evaluate(
            "(()=>{window.__sumiMessages=[];window.__sumiOnMessage=e=>{"
            f"if(e.data?.sumi==={json.dumps(self.nonce)})window.__sumiMessages.push("
            f"{{origin:e.origin,fromPopup:e.source===globalThis[{json.dumps(ref)}],data:e.data}})"
            "};addEventListener('message',window.__sumiOnMessage);"
            f"globalThis[{json.dumps(ref)}]=window.open({json.dumps(url)},{json.dumps(ref)},"
            "'popup,width=600,height=500');"
            f"return Boolean(globalThis[{json.dumps(ref)}])}})()",
            True)
        require(state["accepted"] is True, "Popup request must be accepted")
        target = {}

        def appeared():
            found = [item for item in page_targets(work) if item["targetId"] != work.target["targetId"]]
            require(len(found) <= 1, "Unexpected popup family")
            target.update(next((item for item in found
                                if item.get("openerId") == work.target["targetId"]), {}))
            return bool(target)
        self.settled(appeared, "Popup did not appear in Work's own browser process")
        state["target"] = {"targetId": target["targetId"], "openerId": target.get("openerId"),
                           "url": target.get("url")}
        require([item["targetId"] for item in page_targets(personal)] == [personal.target["targetId"]],
                "The popup must not appear in Personal")
        session = work.cdp.command("Target.attachToTarget",
                                   {"targetId": target["targetId"], "flatten": True})["sessionId"]

        def evaluate(expression, gesture=False):
            value = work.cdp.command("Runtime.evaluate",
                                     {"expression": expression, "returnByValue": True,
                                      "awaitPromise": True, "userGesture": gesture}, session)
            require(not value.get("exceptionDetails"), json.dumps(value.get("exceptionDetails"))[:1000])
            return value["result"].get("value")

        self.popup_loaded(evaluate, url)
        state["context"] = evaluate(
            "({href:location.href,origin:location.origin,secure:isSecureContext,"
            "opener:window.opener!==null,openerOrigin:window.opener?.location.origin,"
            "tauri:typeof __TAURI_INTERNALS__,ipc:typeof window.ipc,"
            "wry:typeof window.webkit?.messageHandlers?.ipc,webview2:typeof window.chrome?.webview})")
        require(state["context"]["origin"] == self.site, "The popup must load the fixture origin")
        require(state["context"]["secure"] is True, "The popup document must be a secure context")
        require(state["context"]["opener"] is True and state["context"]["openerOrigin"] == self.site,
                "The popup must keep Work as its opener")
        for field in ("tauri", "ipc", "wry", "webview2"):
            require(state["context"][field] == "undefined",
                    f"A CEF popup must not see the App's {field} bridge")
        state["sharedBefore"] = evaluate(
            f"({{storage:localStorage.getItem({key}),cookie:{self.cookie_of()}}})")
        require(state["sharedBefore"] == {"storage": expected[0]["storage"], "cookie": expected[0]["cookie"]},
                "The popup must share Work's storage and cookie")
        evaluate(
            f"(()=>{{localStorage.setItem({key},{json.dumps(result)});"
            f"document.cookie={json.dumps(self.key + '=' + result + '; Max-Age=86400; Path=/; Secure; SameSite=Lax')};"
            f"window.opener.postMessage({{sumi:{json.dumps(self.nonce)},result:{json.dumps(result)}}},"
            f"{json.dumps(self.site)});setTimeout(()=>window.close(),0);return true}})()",
            True)

        def closed_back():
            return (work.evaluate(f"globalThis[{json.dumps(ref)}].closed") is True
                    and not any(item["targetId"] == target["targetId"] for item in page_targets(work)))
        self.settled(closed_back, "Popup did not close back to Work")
        state["messages"] = work.evaluate("window.__sumiMessages")
        require(state["messages"] == [{"origin": self.site, "fromPopup": True,
                                       "data": {"sumi": self.nonce, "result": result}}],
                "Work must receive exactly its popup's result")
        state["workAfter"] = work.evaluate(
            f"({{storage:localStorage.getItem({key}),cookie:{self.cookie_of()}}})")
        require(state["workAfter"] == {"storage": result, "cookie": result},
                "Work must see the state its popup wrote")
        require(self.sample(personal) == expected[1],
                "Personal must see neither the popup nor its state")
        # Restore Work's first-launch state, which the later phases check.
        work.evaluate(
            f"(()=>{{localStorage.setItem({key},{json.dumps(expected[0]['storage'])});"
            f"document.cookie={json.dumps(self.key + '=' + expected[0]['cookie'] + '; Max-Age=86400; Path=/; Secure; SameSite=Lax')};"
            "removeEventListener('message',window.__sumiOnMessage);delete window.__sumiOnMessage;"
            f"delete window.__sumiMessages;delete globalThis[{json.dumps(ref)}];return true}})()")
        require(self.sample(work) == expected[0], "Work must keep its document after the popup")
        state["passed"] = True
        return state

    def open_popup_for_quit(self, page):
        """A popup left open when the App quits closes with its profile: the
        native close chain covers it, and the runner then requires a normal
        guard and App exit. Quit only after the popup's own document
        loaded, so its native browser exists in the profile's close
        chain. Mirrors the Linux runner's openPopupForQuit."""
        ref = f"sumi_quit_popup_{self.nonce.replace('-', '')}"
        url = f"{self.site}/?{ref}"
        accepted = page.evaluate(
            f"(()=>{{globalThis[{json.dumps(ref)}]=window.open({json.dumps(url)},{json.dumps(ref)},"
            "'popup,width=500,height=400');"
            f"return Boolean(globalThis[{json.dumps(ref)}])}})()",
            True)
        require(accepted is True, "Popup request must be accepted")
        target = {}

        def appeared():
            found = [item for item in page.cdp.command("Target.getTargets")["targetInfos"]
                     if item["type"] == "page" and item.get("openerId") == page.target["targetId"]]
            target.update(found[0] if found else {})
            return bool(target)
        self.settled(appeared, "Popup did not appear before the final quit")
        session = page.cdp.command("Target.attachToTarget",
                                   {"targetId": target["targetId"], "flatten": True})["sessionId"]

        def evaluate(expression):
            value = page.cdp.command("Runtime.evaluate", {"expression": expression, "returnByValue": True},
                                     session)
            require(not value.get("exceptionDetails"), json.dumps(value.get("exceptionDetails"))[:1000])
            return value["result"].get("value")

        self.popup_loaded(evaluate, url)
        observed = evaluate("({navigation:performance.getEntriesByType('navigation')[0]?.name??null,"
                            "href:location.href,ready:document.readyState})")
        return {"targetId": target["targetId"], "openerId": target.get("openerId"),
                "profile": page.process["profile"], "requested": url, "observed": observed}

    def first_launch(self):
        win = self.win
        opened = self.proof["opened"] = self.marker("app-browser-process-open.json")
        require(len(opened["processes"]) == 2, "Expected two browser processes")
        home = opened["homeWindow"]
        require(win.owner(home)[0] == self.app_pid and win.top_level(home),
                "The home window must be the App's own top-level window")
        pages = [self.attach(self.process_of(opened, name)) for name in ("Work", "Personal")]
        work, personal = pages
        for page, user in ((work, "work"), (personal, "personal")):
            self.sign_in(page, user, open_app=self.real is None)
        if self.real:
            self.open_real_site(pages)
        self.step("separate initial state")
        key = json.dumps(self.key)
        for index, page in enumerate(pages):
            require(page.evaluate(f"localStorage.getItem({key})") is None, "A new profile has no storage")
            require(page.evaluate(self.cookie_of()) is None, "A new profile has no script cookie")
            value = f"{index}:{self.nonce}"
            page.evaluate(f"(()=>{{globalThis[{key}]={json.dumps(value)};localStorage.setItem({key},globalThis[{key}]);"
                          f"document.cookie={json.dumps(f'{self.key}={value}; Max-Age=86400; Path=/; Secure; SameSite=Lax')}"
                          ";return true})()")
        expected = self.proof["initial"] = [self.sample(page) for page in pages]
        require(expected[0]["storage"] != expected[1]["storage"], "Each profile must keep its own storage")
        require([item["cookie"] for item in expected] == [f"0:{self.nonce}", f"1:{self.nonce}"],
                "Each profile must see only its own cookie")
        if self.real:
            # The sign-in was checked on the fixture home; the real site is
            # anonymous, so only its origin is the same in both.
            require([item["origin"] for item in expected] == [self.app_origin] * 2,
                    f"Both profiles must be on {self.app_origin}")
        else:
            require([item["user"] for item in expected] == ["work", "personal"],
                    "Each profile must keep its own signed-in session")
        require(work.native["nativePid"] != personal.native["nativePid"] and work.window != personal.window,
                "Each connection must have its own browser process and window")
        bindings = [{"native": page.native, "context": page.context} for page in pages]
        if self.real:
            # The real site's own context, and how its editor was entered.
            for binding, page in zip(bindings, pages):
                binding["siteContext"] = page.site_context
        self.case("separate-profiles", {"bindings": bindings})

        # The home comes back through the OS's window switching (the owner's
        # decision: no in-browser home button). A browser window is in front
        # first, so the switch is real; Work must not be in front, so the
        # reopen below has to raise it (and, minimized, restore it).
        self.step("home through OS window switching")
        switch = {"precondition": win.switch_to(personal.window, "Personal's window", change=False)}
        self.visibility(personal, "personal-in-front")
        # Native occlusion is off in the Windows browser (process_main.rs).
        # With it on, Work covered by Personal at the same rectangle reported
        # `hidden` (run 42298520970); a renamed or ignored feature would bring
        # that back silently, since the reuse below starts from a minimized
        # Work. So the covered Work must report `visible` here, and the cover
        # must be real for that to mean anything.
        named = {"home": home, "work": work.window, "personal": personal.window}
        # If Personal was only now switched to the front, an occlusion update
        # could take a moment: watch Work's page through that window of time.
        route = switch["precondition"]["route"]
        samples = []
        if route != "already":
            deadline = time.monotonic() + OCCLUSION_WATCH
            while time.monotonic() < deadline:
                samples.append(work.evaluate("document.visibilityState"))
                time.sleep(OCCLUSION_INTERVAL)
            require("hidden" not in samples,
                    f"Work, covered by Personal, read hidden after the switch ({samples}): Chromium's native "
                    "occlusion is not off")
        covered = self.visibility(work, "personal-in-front")
        front = self.proof["visibility"]["personal-in-front"][0]["window"].get("rect")
        under = covered["window"].get("rect")
        order = self.proof.setdefault("zOrder", {})["personal-in-front"] = win.z_order(named)
        require(front and under and front[0] <= under[0] and front[1] <= under[1] and front[2] >= under[2]
                and front[3] >= under[3] and order["positions"]["personal"] is not None
                and order["positions"]["work"] is not None
                and order["positions"]["personal"] < order["positions"]["work"],
                f"Personal does not cover Work (rects {front} over {under}, Z-order {order['positions']}), so "
                "Work's visibility would not show native occlusion off")
        require(covered.get("visibility") == "visible",
                "Work, covered by Personal, reports hidden: Chromium's native occlusion is not off")
        switch["coveredWorkVisible"] = True
        switch["coveredWorkRoute"] = route
        switch["coveredWorkSamples"] = len(samples)
        switch["home"] = win.switch_to(home, "The home window")
        require(win.foreground() == home and win.visible(home), "The home window must be in front and visible")
        switch["lockTimeout"] = win.lock_timeout()
        switch["lock"] = win.foreground_lock()
        switch["homeWindow"] = win.window_state(home)
        self.case("home-by-window-switch", switch)
        # The Windows browser runs without Chromium's native occlusion
        # tracking (process_main.rs), so a covered window stays visible; a
        # minimized one is still hidden. Minimize Work from here (no input),
        # so the reopen below must restore it and make its page visible
        # again. A desktop where that does not hide the page fails here, with
        # its evidence.
        self.step("Work minimized before the reopen", work)
        win.minimize(work.window)
        self.settled(lambda: win.iconic(work.window), "Work's window did not minimize")
        try:
            self.settled(lambda: work.evaluate("document.visibilityState") == "hidden", "not hidden")
        except Failure:
            self.visibility(work, "minimized")
            raise Failure("Minimizing Work's window did not make its page hidden on this desktop, so the reopen "
                          "cannot show it visible again") from None
        require(win.foreground() == home, f"The home must stay in front while Work is minimized ({win.foreground()})")
        self.visibility(work, "before-reuse")
        self.proof["zOrder"]["before-reuse"] = win.z_order(named)
        self.checkpoint("inspect-ready")

        self.step("home reopens Work")
        resumed = self.proof["resumed"] = self.marker("app-browser-process.json")
        require([self.sample(page) for page in pages] == expected,
                "Reopening from the home must retain both exact documents and storage")
        require(sorted(item["opened"]["nativePid"] for item in resumed["after"])
                == sorted(page.native["nativePid"] for page in pages) and len(resumed["after"]) == 2,
                "Reopening must not start another browser process")
        require(win.wait_foreground(work.window), f"Reopened Work's window is not in front ({win.foreground()})")
        require(not win.iconic(work.window), "Reopened Work's window is still minimized")
        require(work.pages() == [work.target["targetId"]], "Reopening must not open another window")
        refusals = self.raise_refusals(work)
        require(refusals == [], f"Work's browser refused the raise: {refusals}")
        # The raise came from a browser that was not in front, while the home
        # was and Windows' foreground lock was active. That is the condition
        # recorded here, not proof that the App's grant was needed: in run
        # 42312377621 the observer, which has no grant, took the foreground
        # from the home with a plain SetForegroundWindow under the same lock.
        # The App's own AllowSetForegroundWindow result is recorded with it.
        lock = switch["lock"]
        require(lock["active"], f"Windows' foreground lock is not active here ({lock})")
        work_after = next(item for item in resumed["after"] if item["profile"] == work.process["profile"])
        evidence = {"window": work.window, "foreground": win.foreground(), "homeRoute": switch["home"]["route"],
                    "lockBefore": lock, "lockAfter": win.foreground_lock(),
                    "foregroundGrantAccepted": work_after.get("foregroundGrant"), "minimizedBefore": True}
        # The page was hidden while minimized; the raise restored the window,
        # so the page must report visible again.
        try:
            self.settled(lambda: work.evaluate("document.visibilityState") == "visible", "not visible")
            evidence["visibility"] = "visible"
        except Failure:
            evidence["visibility"] = "hidden"
        # Keyboard focus is evidence, not a requirement (a documented gap):
        # the browser asks for it again after the raise (process_main.rs), but
        # in CI the page reported document.hasFocus() false while Win32 focus
        # sat on the top-level Chrome widget, which Chrome's views windows
        # keep for toolbar and web contents alike, so the GUI thread's record
        # cannot tell them apart. Both are recorded; the record must exist.
        try:
            self.settled(lambda: work.evaluate("document.hasFocus()") is True, "no focus", 3)
            evidence["focus"] = True
        except Failure:
            evidence["focus"] = False
        evidence["threadFocus"] = win.thread_focus(work.window)
        # Kept in the observation even when a check below fails.
        self.proof.setdefault("evidence", {})["reuse-raises-window"] = evidence
        self.visibility(work, "after-reuse")
        self.visibility(personal, "after-reuse")
        self.proof["zOrder"]["after-reuse"] = win.z_order(named)
        require(evidence["visibility"] == "visible",
                "Reopened Work's page stayed hidden after the raise restored its minimized window")
        require("error" not in evidence["threadFocus"],
                f"Work's GUI thread focus record could not be captured: {json.dumps(evidence['threadFocus'])}")
        self.case("reuse-raises-window", evidence)

        if not self.real:
            # A sign-in style popup on the local fixture site only: the real
            # sites are anonymous and use their own content. The case name
            # sits between reuse and the quit cases in OBSERVED_CASES, from
            # which real-site runs exclude it too.
            self.step("Work same-origin popup", work)
            self.proof["popup"] = self.work_popup_probe(work, personal, expected)
            self.case("work-same-origin-popup", self.proof["popup"])

        # Chrome's real beforeunload path needs sticky user activation. CDP's
        # userGesture is synthetic and is reported as such, not a real click.
        self.step("Work beforeunload setup", work)
        unload = work.cdp.mark()
        work.evaluate("(()=>{window.__sumiBeforeUnload=e=>{e.preventDefault();e.returnValue='';};"
                      "addEventListener('beforeunload',window.__sumiBeforeUnload);return true})()", True)
        # Sequential App quit (owner's decision): profiles close oldest first, so
        # Work closes first; cancelling its beforeunload stops the quit and
        # Personal is never asked to close.
        personal_mark = personal.cdp.mark()
        self.checkpoint("request-exit")
        self.step("Work beforeunload on sequential quit", work)
        dialog = self.dialog(work, unload, "beforeunload", "The beforeunload dialog did not open")
        self.proof["dialog"] = {"type": dialog["params"]["type"],
                                "origin": url_origin(dialog["params"].get("url") or "")}
        require(self.proof["dialog"]["origin"] == self.app_origin, f"The dialog must belong to {self.app_origin}")
        personal.cdp.drain()
        require(not any(event["method"].startswith("Page.javascriptDialog")
                        for event in personal.cdp.since(personal_mark)),
                "Personal must not be asked to close before Work agrees")
        require(not self.guard_ended(personal.process["guardReceipt"]),
                "Personal must still be running during Work's dialog")
        self.proof["dialogClosedBy"] = self.cancel_dialog(work, unload, "Work's beforeunload cancel was not observed",
                                                          dialog, [(personal, personal_mark)])
        require([self.sample(page) for page in pages] == expected, "A cancelled sequential quit must keep every document")
        self.step("sequential quit stopped at the cancel", work)
        self.checkpoint("cancel-inspected")
        self.proof["blocked"] = self.marker("app-browser-process-blocked.json")
        require(len(self.proof["blocked"]) == 2, "Both profiles must remain after the cancelled quit")
        self.proof["cancelled"] = self.marker("app-browser-process-cancelled.json")
        require([self.sample(page) for page in pages] == expected, "The documents must survive the cancelled quit")
        work.evaluate("(()=>{removeEventListener('beforeunload',window.__sumiBeforeUnload);"
                      "delete window.__sumiBeforeUnload;return true})()")
        self.case("quit-cancel-stops", {"dialog": self.proof["dialog"], "dialogClosedBy": self.proof["dialogClosedBy"]})

        # The accepted sequential trade-off: Work (oldest) now agrees to leave
        # and closes normally first; Personal cancels, so the quit stops with
        # Work closed and Personal kept.
        self.step("Personal cancels after Work closed", personal)
        personal_unload = personal.cdp.mark()
        work_unload = work.cdp.mark()
        personal.evaluate("(()=>{window.__sumiBeforeUnload=e=>{e.preventDefault();e.returnValue='';};"
                          "addEventListener('beforeunload',window.__sumiBeforeUnload);return true})()", True)
        self.checkpoint("request-partial-exit")
        opened = None
        try:
            opened = self.dialog(personal, personal_unload, "beforeunload",
                                 "Personal's beforeunload did not open after Work closed", END_WAIT)
            # The App drops Work only after its normal receipt, before asking Personal.
            require(self.guard_ended(work.process["guardReceipt"]), "Work must have closed before Personal is asked")
            self.proof["workClosedFirst"] = self.normal_receipt(work.process["guardReceipt"],
                                                                "Work must have closed normally")
            # As Work's: the dialog is the page's own, not a subframe's.
            self.proof["partialDialog"] = {"type": opened["params"]["type"],
                                           "origin": url_origin(opened["params"].get("url") or "")}
            require(self.proof["partialDialog"]["origin"] == self.app_origin,
                    f"Personal's dialog must belong to {self.app_origin}")
        except Failure as error:
            self.dialog_failure("Personal's beforeunload after Work closed",
                                [(personal, personal_unload), (work, work_unload)], error)
            raise
        closed_by = self.cancel_dialog(personal, personal_unload, "Personal's beforeunload cancel was not observed",
                                       opened, [(work, work_unload)])
        require(self.sample(personal) == expected[1], "The cancelled Personal must keep its document")
        self.checkpoint("partial-inspected")
        self.proof["partial"] = self.marker("app-browser-process-partial.json")
        require([item["profile"] for item in self.proof["partial"]] == [personal.process["profile"]],
                "Only Personal remains after Work closed and Personal cancelled")
        personal.evaluate("(()=>{removeEventListener('beforeunload',window.__sumiBeforeUnload);"
                          "delete window.__sumiBeforeUnload;return true})()")
        self.case("quit-partial-order", {"workReceipt": self.proof["workClosedFirst"],
                                         "dialog": self.proof["partialDialog"], "dialogClosedBy": closed_by})
        # The opted-in browser holds Personal's permission prompt for the
        # App's own answer: this run's checkpoint hands it to the driver,
        # whose click on the home row grants it before the final quit.
        self.answer_permission(personal)
        self.media_dismissal(personal)
        self.step("Personal final exit", personal)
        if not self.real:
            # Personal's own popup stays open for the final quit: its native
            # browser is inside the profile's close chain, and the run's
            # normal receipts cover it (mirrors the Linux runner).
            self.proof["finalPopup"] = self.open_popup_for_quit(personal)
        self.checkpoint("request-final-exit")
        if self.real:
            self.finish_final_quit([personal])
        self.proof["passed"] = True

    def relaunch(self, kind):
        """A later launch on the same App data. restart: each profile's
        storage, script cookie and signed-in session from the first launch
        survive and stay separate. reset: Work was cleared from the home, so
        only Personal keeps its state, and a clear of the running Personal
        was refused."""
        self.proof["scope"] = f"Disposable Windows actual App {kind} on the same data; local fixture site"
        opened = self.proof["opened"] = self.marker(f"app-browser-process-{kind}-open.json")
        pages = [self.attach(self.process_of(opened, name)) for name in ("Work", "Personal")]
        work, personal = pages
        self.step(f"per-profile state after {kind}")
        persisted = self.proof["persisted"] = [self.persisted(page) for page in pages]
        kept = {"storage": f"1:{self.nonce}", "cookie": f"1:{self.nonce}", "page": "home", "user": "personal"}
        if kind == "reset":
            require(persisted == [{"storage": None, "cookie": None, "page": "login", "user": None}, kept],
                    f"Only Work's site data may be cleared: {persisted}")
            require(opened["runningClearRefused"] is True, "Clearing a running profile must be refused")
            require(opened["personalLockHeld"] is True, "Chromium must hold the running Personal's lockfile")
            self.case("clear-only-one-profile", {"persisted": persisted})
        else:
            require(persisted == [{"storage": f"0:{self.nonce}", "cookie": f"0:{self.nonce}", "page": "home",
                                   "user": "work"}, kept],
                    f"Each profile must keep its own storage, cookie and session across App restart: {persisted}")
            self.case("restart-keeps-separation", {"persisted": persisted})
        self.checkpoint(f"{kind}-inspected")
        self.step(f"Personal native close after {kind}", personal)
        self.win.close(personal.window)
        self.proof["personalClosed"] = self.normal_receipt(personal.process["guardReceipt"],
                                                           f"Personal guard did not finish after {kind}", END_WAIT)
        self.step(f"Work ordinary exit after {kind}", work)
        self.checkpoint(f"{kind}-request-exit")
        self.proof["passed"] = True

    def failure_capture(self):
        try:
            window = self.win.foreground()
            self.proof["failureForeground"] = {"window": window, "owner": self.win.owner(window)[0] if window else 0}
        except Exception as error:  # diagnostics only
            self.proof["failureForeground"] = {"error": str(error)}

    def recovery(self):
        """Fresh-case, same-running-App recovery, with one explicit owned fault."""
        require(not self.real and self.installed is None and self.recovery_images and self.recovery_app_data,
                "Recovery requires the standalone synthetic-site fixture and exact process images")
        api = WindowsProcesses(self.win.kernel32)
        initial = self.proof["opened"] = self.marker("app-browser-process-recover-initial.json")
        work, personal = [self.attach(self.process_of(initial, name)) for name in ("Work", "Personal")]

        def held(process, allow_fault=False):
            return OwnedBrowser(api, {"app": self.app_pid, "guard": process["pid"],
                                     "browser": process["opened"]["nativePid"]},
                                self.recovery_images, allow_fault)

        def data(page):
            value = self.sample(page)
            return {name: value.get(name) for name in ("document", "storage", "cookie", "user")}

        def cache_identity(process):
            paths = recovery_paths(process, self.recovery_app_data, self.data / "r")
            result = {}
            for name in ("root", "cache"):
                path = paths[name]
                info = path.lstat()
                require(path.is_dir() and not path.is_symlink() and info.st_ino > 0
                        and not (getattr(info, "st_file_attributes", 0) & 0x400),
                        "Plain identifiable cache directory required")
                result[name] = {"device": info.st_dev, "inode": info.st_ino}
            return result

        with held(personal.process) as other:
            for index, page in enumerate((work, personal)):
                self.sign_in(page, ("work", "personal")[index])
                key, value = json.dumps(self.key), json.dumps(f"{index}:{self.nonce}")
                cookie = json.dumps(f"{self.key}={index}:{self.nonce}; Max-Age=86400; Path=/; Secure; SameSite=Lax")
                page.evaluate(f"(()=>{{globalThis[{key}]={value};localStorage.setItem({key},{value});"
                              f"document.cookie={cookie};return true}})()")
            expected = [data(page) for page in (work, personal)]
            require([value["cookie"] for value in expected] == [f"0:{self.nonce}", f"1:{self.nonce}"],
                    "Both recovery seeds must exist in their separate profiles")
            self.win.close(work.native["windowId"])
            seed_receipt = self.normal_receipt(work.process["guardReceipt"], "Work seed must close normally")
            self.proof["seedNormal"] = seed_receipt
            self.checkpoint("recover-seeded")

            before = self.marker("app-browser-process-recover-before.json")
            crashed = self.attach(self.process_of(before, "Work"))
            saved = dict(expected[0], document=None)
            require(data(crashed) == saved, "Recovery input must be durable before the induced crash")
            require(data(personal) == expected[1], "Personal changed before the induced crash")
            cache_before = cache_identity(crashed.process)
            with held(crashed.process, allow_fault=True) as target:
                self.proof["fault"] = target.fault()
                self.settled(lambda: self.guard_ended(crashed.process["guardReceipt"]),
                             "The exact crash guard must finish", 30)
                target.guard_ended()
                receipt = json.loads(Path(crashed.process["guardReceipt"]).read_bytes())
                require(type(receipt.get("rootExit")) is int and receipt["rootExit"] == FAULT_EXIT
                        and receipt.get("complete") is True and receipt.get("kernelNoChildren") is True
                        and receipt.get("remaining") == [] and receipt.get("cleanupErrors") == []
                        and receipt.get("nonFatalErrors") == [] and receipt.get("error") is None
                        and type(receipt.get("forcedCleanup")) is int and receipt["forcedCleanup"] >= 0,
                        "The injected failure must have an exact, conclusive abnormal receipt")
                self.proof["injectedAbnormal"] = receipt
                other.validate()
                self.checkpoint("recover-crashed")

                offered = self.marker("app-browser-process-recover-offered.json")
                require(offered == before["processes"], "Ordinary open must only offer recovery")
                require(data(personal) == expected[1], "Offering recovery changed Personal")
                self.case("recovery-requires-consent")
                self.checkpoint("recover-consent")
                reopened = self.marker("app-browser-process-recover-open.json")
                require(reopened.get("explicitRecovery") is True and reopened["connections"] == before["connections"],
                        "Explicit recovery must retain the exact saved connections")
                fresh = self.process_of(reopened, "Work")
                require(self.process_of(reopened, "Personal") == self.process_of(before, "Personal"),
                        "Personal's native ownership changed during recovery")
                require(fresh["profile"] == crashed.process["profile"]
                        and fresh["directory"] == crashed.process["directory"]
                        and fresh["pid"] != crashed.process["pid"]
                        and fresh["opened"]["nativePid"] != crashed.native["nativePid"],
                        "Recovery must reuse the profile/directory with new native processes")
                recovered = self.attach(fresh)
                with held(fresh) as replacement:
                    require(all(replacement.identities[role]["created"] >= target.identities[role]["created"]
                                for role in ("guard", "browser")), "Replacement generation predates the crash")
                    require(data(recovered) == saved and cache_identity(fresh) == cache_before,
                            "Recovery replaced the saved data or cache directories")
                    self.case("recovery-keeps-settled-data")
                    other.validate()
                    require(data(personal) == expected[1], "Recovery changed Personal's document or data")
                    self.case("recovery-preserves-other-profile")
                    incidents = list((Path(fresh["directory"]) / "recoveries").iterdir())
                    require(len(incidents) == 1, "One recovery incident must retain the abnormal generation")
                    incident_file = incidents[0]
                    info = incident_file.lstat()
                    require(incident_file.is_file() and not incident_file.is_symlink()
                            and not (getattr(info, "st_file_attributes", 0) & 0x400)
                            and info.st_nlink == 1 and 0 < info.st_size <= 1024 * 1024,
                            "A bounded plain recovery incident is required")
                    incident = json.loads(incident_file.read_bytes())
                    require(incident.get("schema") == 1 and incident.get("profile") == fresh["profile"]
                            and incident.get("guardPid") == crashed.process["pid"]
                            and incident.get("nativePid") == crashed.native["nativePid"]
                            and incident.get("receipt") == receipt,
                            "The original abnormal receipt must survive unchanged in the incident")
                    self.proof["incident"] = incident
                    self.case("recovery-retains-abnormal-receipt")
                    for page in (recovered, personal):
                        self.sandbox(page.process, "recovery-final")
                    replacement.validate()
                    self.proof["reopened"] = reopened
                    self.proof["cacheIdentity"] = cache_before
                    self.checkpoint("recover-inspected")
        self.proof["passed"] = True

    def run(self):
        try:
            if self.phase == "recover":
                self.recovery()
            elif self.phase:
                self.relaunch(self.phase)
            else:
                self.first_launch()
        except Exception as error:
            self.proof["error"] = f"{type(error).__name__}: {error}"
            self.failure_capture()
            raise
        finally:
            for client in self.clients:
                try:
                    client.websocket.close()
                except OSError:
                    pass
            with open(self.data / f"browser-observation{self.suffix}.json", "w", encoding="utf-8") as output:
                output.write(json.dumps(self.proof, indent=2) + "\n")


def main():
    # Logs and App messages may hold text the console's code page lacks.
    for stream in (sys.stdout, sys.stderr):
        stream.reconfigure(errors="backslashreplace")
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--data", type=Path, required=True, help="the runner's session directory")
    parser.add_argument("--site", required=True, help="the fixture site origin")
    parser.add_argument("--app-pid", type=int, required=True)
    parser.add_argument("--phase", default="", choices=["", "restart", "reset", "recover"])
    for role in ("app", "guard", "browser"):
        parser.add_argument(f"--recovery-{role}", type=Path)
    parser.add_argument("--recovery-app-data", type=Path)
    parser.add_argument("--real-site", choices=sorted(REAL_SITES), help="W4: the app link's real site")
    parser.add_argument("--installed", type=Path,
                        help="W6: the installed App's directory; its guards and browsers must run from its cef")
    parser.add_argument("--unsandboxed-candidate", action="store_true",
                        help="the browser is the unsandboxed candidate: record its sandbox evidence, do not require it")
    args = parser.parse_args()
    if args.real_site and args.phase:
        parser.error("--real-site runs the first launch only")
    recovery_images = {role: str(getattr(args, f"recovery_{role}")) for role in ("app", "guard", "browser")}
    if args.phase == "recover" and (args.installed or args.real_site or args.recovery_app_data is None or
                                    any(getattr(args, f"recovery_{role}") is None for role in recovery_images)):
        parser.error("--phase recover needs exact App/guard/browser paths and a standalone synthetic site")
    if args.phase != "recover":
        if args.recovery_app_data is not None or any(getattr(args, f"recovery_{role}") is not None
                                                   for role in recovery_images):
            parser.error("Recovery process paths are only accepted with --phase recover")
        recovery_images = None
    if os.name != "nt":
        parser.error("windows_browser_observer.py runs on Windows")
    data = args.data.resolve(strict=True)
    require((data / ".owned-browser-reference").is_file(), "No owned session directory")
    Observer(data, args.site, args.app_pid, args.phase, Win32(), args.real_site, args.installed,
             expect_sandbox=not args.unsandboxed_candidate, recovery_images=recovery_images,
             recovery_app_data=args.recovery_app_data).run()


if __name__ == "__main__":
    main()
