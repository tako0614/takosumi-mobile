"""Two disposable HTTPS origins for actual App UI; no native replacement UI."""
import html
import http.server
from http.cookies import SimpleCookie
import secrets
import ssl
import threading
from urllib.parse import urlsplit


class Site(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, certificate, key, kind):
        if kind not in ("instance", "editor"):
            raise ValueError("Closed synthetic site kind required")
        self.kind = kind
        self.tokens = {user: secrets.token_hex(24) for user in ("Work", "Personal")}
        self.events = []
        self.editor = None
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(certificate, key)
        super().__init__(("127.0.0.1", 0), Handler)
        try:
            self.socket = context.wrap_socket(self.socket, server_side=True)
        except BaseException:
            self.server_close()
            raise
        self.thread = threading.Thread(target=self.serve_forever, daemon=True)

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

    def reply(self, status, body, **headers):
        data = body.encode()
        self.send_response(status)
        for key, value in {"Content-Type": "text/html; charset=utf-8", "Content-Length": str(len(data)),
                           "Cache-Control": "no-store", "Connection": "close", **headers}.items():
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        path = urlsplit(self.path).path
        cookies = SimpleCookie()
        cookies.load(self.headers.get("Cookie", ""))
        token = cookies.get("fixture_session")
        user = next((name for name, value in self.server.tokens.items()
                     if token is not None and secrets.compare_digest(token.value, value)), None)
        allowed = {"/", "/account", "/sign-in/Work", "/sign-in/Personal", "/editor", "/editor/page-two"}
        self.server.events.append({"site": self.server.kind, "path": path if path in allowed else "other",
                                   "user": user, "cookiePresent": token is not None})
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
        except (BrokenPipeError, ConnectionResetError, ssl.SSLError):
            pass
