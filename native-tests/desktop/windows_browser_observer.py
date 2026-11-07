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
from windows_smoke import Failure, require  # noqa: E402

GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
MAX_MESSAGE = 16 * 1024 * 1024
MARKER_WAIT = 30
SETTLE_WAIT = 10
# The fixture site's documents: a navigation within a profile, or the boot
# load of a new browser (the cef-candidate popup probe's 25 s load bound).
LOAD_WAIT = 25
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

    def _fill(self, deadline):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("WebSocket read timed out")
        self.sock.settimeout(remaining)
        chunk = self.sock.recv(65536)
        if not chunk:
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
                        return bool(first & 0x80), first & 0x0F, mask_bytes(payload, mask) if mask else payload
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
        while True:
            fin, opcode, payload = self._frame(deadline)
            if opcode == 0x8:
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

    def mark(self):
        mark = self.base + len(self.events)
        self.marked_at[mark] = time.monotonic()
        return mark

    def since(self, mark):
        return self.events[max(0, mark - self.base):]

    def _next(self, deadline):
        try:
            message = json.loads(self.websocket.receive(deadline))
        except TimeoutError:
            return None
        except (ConnectionError, OSError) as error:
            self.closed = str(error)
            raise Failure(f"DevTools connection of {self.label} ended: {error}") from None
        if "method" in message:
            message[RECEIVED_AT] = time.monotonic()
            self.events.append(message)
            if len(self.events) > 4096:
                del self.events[:1024]
                self.base += 1024
        return message

    def command(self, method, params=None, session=None, timeout=5.0):
        self.next += 1
        ident = self.next
        request = {"id": ident, "method": method, "params": params or {}}
        if session:
            request["sessionId"] = session
        self.websocket.send_text(json.dumps(request))
        deadline = time.monotonic() + timeout
        while True:
            message = self._next(deadline)
            if message is None:
                raise Failure(f"CDP timeout {method} ({self.label})")
            if message.get("id") == ident:
                if "error" in message:
                    raise CdpError(f"CDP {method} failed ({self.label}): {json.dumps(message['error'])[:500]}",
                                   message["error"])
                return message.get("result", {})

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


def module_problems(role, modules, cef):
    """What a W6 process's loaded modules show against the static C runtime
    and the installed bundle: no Visual C++ runtime module in either process,
    and the browser's libcef.dll from the installation's cef directory."""
    if modules is None:
        return [f"no module snapshot of the {role}"]
    problems = [f"the {role} loaded {path}" for path in modules
                if ntpath.basename(path).lower().startswith(CRT_MODULES)]
    if role == "browser":
        libcef = [path for path in modules if ntpath.basename(path).lower() == "libcef.dll"]
        if not libcef:
            problems.append("the browser has not loaded libcef.dll")
        problems += [f"the browser loaded libcef.dll from {path}" for path in libcef if not inside(path, cef)]
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

    def evaluate(self, expression, gesture=False):
        result = self.send("Runtime.evaluate", {"expression": expression, "returnByValue": True,
                                                "awaitPromise": True, "userGesture": gesture})
        require(not result.get("exceptionDetails"), json.dumps(result.get("exceptionDetails"))[:1000])
        return result["result"].get("value")

    def pages(self):
        return [target["targetId"] for target in self.cdp.command("Target.getTargets")["targetInfos"]
                if target["type"] == "page"]


# --- Observation ---------------------------------------------------------------

class Observer:
    def __init__(self, data, site, app_pid, phase, win32=None, real_site=None, installed=None):
        self.data, self.site, self.app_pid, self.phase = data, site, app_pid, phase
        # W6 (--packaged): the installed App's directory; each guard and
        # browser must run from its cef\ (installed_from).
        self.installed = installed
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
        require(value["rootExit"] == 0 and value["complete"] is True and value["kernelNoChildren"] is True
                and value["forcedCleanup"] == 0 and value["remaining"] == [] and value["cleanupErrors"] == []
                and value["error"] is None, f"{message}: abnormal receipt {json.dumps(value)[:500]}")
        return value

    def cookie_of(self):
        key = self.key
        return (f"(document.cookie.split('; ').find(c=>c.startsWith({json.dumps(key + '=')}))"
                f"?.slice({len(key) + 1})??null)")

    def sample(self, page):
        key = json.dumps(self.key)
        return page.evaluate(
            f"({{document:globalThis[{key}],storage:localStorage.getItem({key}),cookie:{self.cookie_of()},"
            "origin:location.origin,frame:window===top,page:document.querySelector('main')?.dataset.page??null,"
            "user:document.querySelector('main')?.dataset.user??null})")

    def persisted(self, page):
        key = json.dumps(self.key)
        return page.evaluate(
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
        problems = [problem for role in pids for problem in module_problems(role, modules[role], cef)]
        libcef = [path for path in modules["browser"] or [] if ntpath.basename(path).lower() == "libcef.dll"]
        self.proof.setdefault("installedImages", []).append(dict(
            images, profile=process["profile"], libcef=libcef,
            modules={role: len(found) if found is not None else None for role, found in modules.items()}))
        for role, image in images.items():
            require(inside(image, cef), f"The {role} of {process['profile']} runs from {image}, not from {cef}")
        require(not problems, f"The installed processes of {process['profile']}: {problems}")

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

    def finish_final_quit(self, pages):
        """W4: after the final quit request, a real site may ask its own
        beforeunload (the quit then waits for an answer). Accept only that
        (final_quit_dialog; recorded, as is a refused one) until every
        remaining profile's guard has ended."""
        accepted = self.proof["finalQuitDialogsAccepted"] = []
        marks = {id(page): page.cdp.mark() for page in pages}
        deadline = time.monotonic() + FINAL_QUIT_WAIT
        remaining = list(pages)
        while remaining:
            require(time.monotonic() < deadline, "The final quit did not end every remaining profile")
            for page in list(remaining):
                if self.guard_ended(page.process["guardReceipt"]):
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
            try:
                page.send("Page.handleJavaScriptDialog", {"accept": False})
            except CdpError as error:
                if not (self.real and opened is not None and error.error.get("message") == NO_DIALOG):
                    raise
                return self.cancelled_by_browser(page, what, opened, attempt, error)
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
        self.step("Personal final exit", personal)
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

    def run(self):
        try:
            if self.phase:
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
    parser.add_argument("--phase", default="", choices=["", "restart", "reset"])
    parser.add_argument("--real-site", choices=sorted(REAL_SITES), help="W4: the app link's real site")
    parser.add_argument("--installed", type=Path,
                        help="W6: the installed App's directory; its guards and browsers must run from its cef")
    args = parser.parse_args()
    if args.real_site and args.phase:
        parser.error("--real-site runs the first launch only")
    if os.name != "nt":
        parser.error("windows_browser_observer.py runs on Windows")
    data = args.data.resolve(strict=True)
    require((data / ".owned-browser-reference").is_file(), "No owned session directory")
    Observer(data, args.site, args.app_pid, args.phase, Win32(), args.real_site, args.installed).run()


if __name__ == "__main__":
    main()
