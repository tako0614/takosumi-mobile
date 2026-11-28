"""Two disposable HTTPS origins for actual App UI; no native replacement UI."""
import html
import http.server
from http.cookies import SimpleCookie
import secrets
import ssl
import sys
import threading
import time
from urllib.parse import urlsplit


def tls_established(connection):
    # This checks negotiation state without another read. The fixture does
    # not request client certificates, so a successful call returns None.
    try:
        connection.getpeercert()
    except ValueError:
        return False
    except OSError:
        return None
    return True


class Site(http.server.ThreadingHTTPServer):
    # server_close waits for the bounded connection workers before retiring TLS.
    daemon_threads = False
    connection_timeout = 5

    def __init__(self, certificate, key, kind):
        if kind not in ("instance", "editor"):
            raise ValueError("Closed synthetic site kind required")
        self.kind = kind
        self.tokens = {user: secrets.token_hex(24) for user in ("Work", "Personal")}
        self.events = []
        self.editor = None
        self._transport_lock = threading.Lock()
        self._transport = {"accepted": 0, "handlerEndedBeforeTls": 0, "handlerEndedAfterTls": 0}
        self._pre_tls_failures = {}
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(certificate, key)
        self.tls_context = context
        super().__init__(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.serve_forever, daemon=True)

    def get_request(self):
        connection, address = super().get_request()
        with self._transport_lock:
            self._transport["accepted"] += 1
        try:
            connection.settimeout(self.connection_timeout)
            # The first read handshakes in the request worker. An unfinished
            # TLS connection must not hold the listener's accept/dispatch loop.
            return self.tls_context.wrap_socket(connection, server_side=True,
                                                do_handshake_on_connect=False), address
        except BaseException:
            connection.close()
            raise

    def record_handler_end(self, connection):
        established = tls_established(connection)
        if established is None:
            return  # Accepted, with no usable observation at handler teardown.
        field = "handlerEndedAfterTls" if established else "handlerEndedBeforeTls"
        with self._transport_lock:
            self._transport[field] += 1

    def record_pre_tls_failure(self, connection, category):
        if tls_established(connection) is False:
            with self._transport_lock:
                self._pre_tls_failures[category] = self._pre_tls_failures.get(category, 0) + 1

    def handle_error(self, request, client_address):
        error = sys.exception()
        if isinstance(error, ssl.SSLError):
            # Keep a closed category, never a peer address or the raw error.
            category = "unknownCaAlert" if error.reason == "TLSV1_ALERT_UNKNOWN_CA" else "otherTlsError"
            self.record_pre_tls_failure(request, category)
        elif isinstance(error, ConnectionResetError):
            self.record_pre_tls_failure(request, "connectionReset")
        super().handle_error(request, client_address)

    def transport_summary(self):
        with self._transport_lock:
            result = self._transport.copy()
            if self._pre_tls_failures:
                result["preTlsFailures"] = self._pre_tls_failures.copy()
            return result

    @property
    def url(self):
        host = "127.0.0.1" if self.kind == "instance" else "localhost"
        return f"https://{host}:{self.server_port}"

    def start(self):
        self.thread.start()

    def stop(self):
        if self.thread.ident is not None:
            self.shutdown()
        self.server_close()
        if self.thread.ident is not None:
            self.thread.join(timeout=5)
        if self.thread.is_alive():
            raise RuntimeError("Owned Web-home site did not stop")


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass  # Never log cookie headers or disposable tokens.

    def log_error(self, message, *args):
        if message == "Request timed out: %r" and args and isinstance(args[0], TimeoutError):
            self.server.record_pre_tls_failure(self.connection, "timeout")
        super().log_error(message, *args)

    def finish(self):
        try:
            super().finish()
        finally:
            self.server.record_handler_end(self.connection)

    def reply(self, status, body, **headers):
        data = body.encode()
        self.record_response(state="writing", status=status, bodyBytes=len(data),
                             responseElapsedMs=self.elapsed_ms())
        self.send_response(status)
        for key, value in {"Content-Type": "text/html; charset=utf-8", "Content-Length": str(len(data)),
                           "Cache-Control": "no-store", "Connection": "close", **headers}.items():
            self.send_header(key, value)
        self.end_headers()
        written = self.wfile.write(data)
        # This proves only that the server-side write returned, never that
        # WebKit committed, painted or exposed the page to accessibility.
        self.record_response(state="written", writtenBytes=written,
                             writeElapsedMs=self.elapsed_ms(), writeReturnedUnixMs=time.time_ns() // 1_000_000)

    def record_response(self, **fields):
        # Request workers may outlive serve_forever's thread. Keep the public
        # request shape fixed and replace a complete response snapshot so JSON
        # collection never traverses a dictionary while this worker grows it.
        self.event["response"] = dict(self.event["response"], **fields)

    def elapsed_ms(self):
        return (time.monotonic_ns() - self.received_ns) / 1_000_000

    def do_GET(self):
        path = urlsplit(self.path).path
        cookies = SimpleCookie()
        cookies.load(self.headers.get("Cookie", ""))
        token = cookies.get("fixture_session")
        user = next((name for name, value in self.server.tokens.items()
                     if token is not None and secrets.compare_digest(token.value, value)), None)
        allowed = {"/", "/account", "/sign-in/Work", "/sign-in/Personal", "/editor", "/editor/page-two"}
        self.received_ns = time.monotonic_ns()
        self.event = {"site": self.server.kind, "path": path if path in allowed else "other",
                      "user": user, "cookiePresent": token is not None,
                      "response": {"state": "received", "receivedUnixMs": time.time_ns() // 1_000_000}}
        self.server.events.append(self.event)
        try:
            if self.server.kind == "instance" and path.startswith("/sign-in/"):
                selected = path.removeprefix("/sign-in/")
                if selected not in self.server.tokens:
                    self.reply(404, "Unknown synthetic account")
                    return
                self.reply(303, "", Location="/", **{"Set-Cookie":
                    f"fixture_session={self.server.tokens[selected]}; Max-Age=86400; Secure; HttpOnly; SameSite=Lax; Path=/"})
                return
            common = ("<!doctype html><meta name='viewport' content='width=device-width'>"
                      "<style>body{font:22px sans-serif;padding:16px}a,label,input{display:block;margin:20px 0}"
                      "input{font:22px sans-serif;max-width:90%}</style>")
            # The page itself observes bridge absence; no test runner injects
            # JS, cookie state or native callbacks into the actual App.
            isolation = ("<p id='bridge'>Bridge check pending</p><script>"
                         "document.getElementById('bridge').textContent="
                         "(!window.__TAURI_INTERNALS__&&!window.__TAURI__&&!window.ipc&&"
                         "!window.webkit?.messageHandlers?.ipc)?"
                         "'Remote has no App IPC':'Unexpected App IPC';</script>")
            if self.server.kind == "instance" and path in ("/", "/account"):
                if user is None:
                    content = ("<h1>Fixture signed out</h1><a href='/sign-in/Work'>Sign in Work</a>"
                               "<a href='/sign-in/Personal'>Sign in Personal</a>")
                elif path == "/account":
                    content = f"<h1>Account settings {user}</h1><a href='/'>Web app home</a>"
                else:
                    if self.server.editor is None:
                        raise RuntimeError("The second owned origin is missing")
                    content = (f"<h1>Fixture home {user}</h1><a href='{html.escape(self.server.editor.url)}/editor'>"
                               "📝 Notes</a><a href='/account'>Account settings</a>"
                               f"<p id='instance-data'></p><script>localStorage.setItem('draft','instance-{user}');"
                               "document.getElementById('instance-data').textContent="
                               "'Instance data '+localStorage.getItem('draft');</script>")
            elif self.server.kind == "editor" and path in ("/editor", "/editor/page-two"):
                title = "Notes page two" if path.endswith("page-two") else "Notes editor"
                content = (f"<h1>{title}</h1><p id='document'></p><label for='draft'>Draft</label>"
                           "<input id='draft' aria-label='Draft' autocomplete='off'>"
                           "<a href='/editor/page-two'>Page two</a><script>"
                           "document.getElementById('document').textContent='Document '+crypto.randomUUID();"
                           "const draft=document.getElementById('draft');draft.value=localStorage.getItem('draft')||'';"
                           "draft.addEventListener('input',()=>localStorage.setItem('draft',draft.value));</script>")
            else:
                self.reply(404, "Unknown fixture page")
                return
            self.reply(200, common + content + isolation)
        except (BrokenPipeError, ConnectionResetError, ssl.SSLError) as error:
            kind = "broken-pipe" if isinstance(error, BrokenPipeError) else (
                "connection-reset" if isinstance(error, ConnectionResetError) else "tls")
            self.record_response(state="transport-failed", transportFailure=kind,
                                 failureElapsedMs=self.elapsed_ms())
