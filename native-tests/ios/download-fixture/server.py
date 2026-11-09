"""Disposable loopback HTTPS site; no native download implementation lives here."""
import http.server
import secrets
import ssl
import threading
from http.cookies import SimpleCookie

COOKIE_BYTES = b"Takosumi same-profile cookie download\n"
BLOB_BYTES = b"Takosumi Blob download\n"
SENTINEL_BYTES = b"unrelated fixture file\n"


class FixtureServer(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, certificate, key):
        self.session = secrets.token_hex(24)
        self.stopping = threading.Event()
        self.events = []
        super().__init__(("127.0.0.1", 0), FixtureHandler)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(certificate, key)
        self.socket = context.wrap_socket(self.socket, server_side=True)
        self.thread = threading.Thread(target=self.serve_forever, daemon=True)

    @property
    def url(self):
        return f"https://127.0.0.1:{self.server_port}/"

    def start(self):
        self.thread.start()

    def stop(self):
        self.stopping.set()
        self.shutdown()
        self.server_close()
        self.thread.join(timeout=5)
        if self.thread.is_alive():
            raise RuntimeError("Owned HTTPS server did not stop")


class FixtureHandler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass  # Never emit the disposable session cookie.

    def reply(self, status, data, content_type="text/html; charset=utf-8", **headers):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("Connection", "close")
        for name, value in headers.items():
            self.send_header(name.replace("_", "-"), value)
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        cookies = SimpleCookie()
        cookies.load(self.headers.get("Cookie", ""))
        token = cookies.get("fixture_session")
        signed_in = token is not None and secrets.compare_digest(token.value, self.server.session)
        # Fixture paths and booleans only, never request headers or cookies.
        path = self.path if self.path in ("/", "/login", "/cookie.txt", "/slow.bin") else "other"
        self.server.events.append({"path": path, "signedIn": signed_in})
        try:
            if self.path == "/login":
                self.reply(303, b"", Location="/", Set_Cookie=(
                    f"fixture_session={self.server.session}; Secure; HttpOnly; SameSite=Lax; Path=/"))
            elif self.path == "/":
                state = "Fixture signed in" if signed_in else "Fixture signed out"
                self.reply(200, ("<!doctype html><meta name='viewport' content='width=device-width'>"
                    "<title>Download fixture</title><style>a{display:block;margin:30px;font-size:22px}</style>"
                    f"<p>{state}</p><a href='/login'>Fixture sign in</a>"
                    "<a href='/cookie.txt'>Cookie attachment</a><a href='/slow.bin'>Slow attachment</a>"
                    "<a id='blob' download='blob.txt'>Blob export</a><script>"
                    "document.getElementById('blob').href=URL.createObjectURL(new Blob("
                    "['Takosumi Blob download\\n'],{type:'text/plain'}));</script>").encode())
            elif self.path == "/cookie.txt":
                if signed_in:
                    self.reply(200, COOKIE_BYTES, "text/plain", Content_Disposition='attachment; filename="cookie.txt"')
                else:
                    self.reply(403, b"<!doctype html><p>Cookie denied</p>")
            elif self.path == "/slow.bin" and signed_in:
                self.send_response(200)
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("Content-Disposition", 'attachment; filename="slow.bin"')
                self.send_header("Content-Length", str(512 * 8192))
                self.send_header("Connection", "close")
                self.end_headers()
                sent = 0
                interrupted = False
                try:
                    for _ in range(512):
                        self.wfile.write(b"s" * 8192)
                        self.wfile.flush()
                        sent += 1
                        if self.server.stopping.wait(0.2):
                            break
                except (BrokenPipeError, ConnectionResetError, ssl.SSLError):
                    interrupted = True
                finally:
                    self.server.events.append({"path": "/slow.bin", "ended": True, "chunks": sent,
                                               "peerClosed": interrupted, "completed": sent == 512})
            else:
                self.reply(404, b"Not found")
        except (BrokenPipeError, ConnectionResetError, ssl.SSLError):
            pass  # A real WebKit cancel closes the owned response socket.
