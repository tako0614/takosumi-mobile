#!/usr/bin/env python3
"""Run the full bundled App against disposable mock Web login/home/app sites."""

import argparse
import http.server
from http.cookies import SimpleCookie
import os
from pathlib import Path
import ssl
import subprocess
import tempfile
import threading
from urllib.parse import parse_qs


class Site(http.server.BaseHTTPRequestHandler):
    def send(self, body, status=200, headers=None):
        data = body.encode()
        self.send_response(status)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.server.role == "app":
            self.send("""<!doctype html><title>Mock editor</title>
                <main data-page="editor"><textarea></textarea><span id="saved"></span></main>
                <script>
                const input=document.querySelector('textarea');
                input.value=localStorage.getItem('draft') || '';
                input.addEventListener('input', () => {
                  localStorage.setItem('draft', input.value);
                  document.querySelector('#saved').dataset.saved='yes';
                });
                </script>""")
            return
        cookies = SimpleCookie(self.headers.get("Cookie", ""))
        identity = cookies.get("fixture_session")
        user = identity.value if identity else ""
        if user not in ("work", "personal"):
            self.send("""<!doctype html><title>Mock login</title><main data-page="login">
                <p>Synthetic fixture only; no real credentials.</p>
                <form method="post" action="/session"><input name="user"><button>Log in</button></form>
                </main>""")
        else:
            self.send(f"""<!doctype html><title>Mock home</title>
                <main data-page="home" data-user="{user}"><h1>{user} apps</h1>
                <a data-app="editor" href="{self.server.app_origin}/">Editor</a></main>""")

    def do_POST(self):
        if self.server.role != "sumi" or self.path != "/session":
            self.send_error(404)
            return
        data = self.rfile.read(int(self.headers.get("Content-Length", "0"))).decode()
        user = parse_qs(data).get("user", [""])[0]
        if user not in ("work", "personal"):
            self.send_error(400)
            return
        self.send("", status=303, headers={
            "Location": "/",
            "Set-Cookie": f"fixture_session={user}; Path=/; Max-Age=86400; Secure; HttpOnly; SameSite=Lax",
        })

    def log_message(self, *_args):
        pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--data", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    data = args.data.absolute()
    if data.exists():
        parser.error("--data must be a new disposable directory")
    data.mkdir(parents=True, mode=0o700)
    with tempfile.TemporaryDirectory(prefix="sumi-full-app-tls-", dir=data) as temporary:
        cert = Path(temporary) / "cert.pem"
        key = Path(temporary) / "key.pem"
        subprocess.run([
            "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
            "-keyout", str(key), "-out", str(cert), "-days", "1",
            "-subj", "/CN=127.0.0.1",
            "-addext", "subjectAltName=IP:127.0.0.1,IP:127.0.0.2",
        ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain(cert, key)
        servers = []
        for role, host in (("app", "127.0.0.2"), ("sumi", "127.0.0.1")):
            server = http.server.ThreadingHTTPServer((host, 0), Site)
            server.role = role
            server.socket = tls.wrap_socket(server.socket, server_side=True)
            servers.append(server)
        servers[1].app_origin = f"https://127.0.0.2:{servers[0].server_port}"
        env = dict(os.environ, TAKOSUMI_FIXTURE_CERT=str(cert),
                   TAKOSUMI_FIXTURE_HTTPS=f"https://127.0.0.1:{servers[1].server_port}")
        # Scope all native plugin/shell/engine state, including cache and portal
        # helpers, to this runner. Never use an installed App's data or keyring.
        for variable, name in (("XDG_DATA_HOME", "data"), ("XDG_CONFIG_HOME", "config"),
                               ("XDG_CACHE_HOME", "cache"), ("XDG_RUNTIME_DIR", "runtime")):
            directory = data / name
            directory.mkdir(mode=0o700)
            env[variable] = str(directory)
        for server in servers:
            threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            for flags in ([], ["--readback"]):
                result = subprocess.run([
                    "xvfb-run", "-a", "dbus-run-session", "--", str(binary), *flags,
                ], env=env, timeout=75, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
                text = result.stdout
                (data / ("readback.log" if flags else "first.log")).write_text(text)
                print(text, end="", flush=True)
                if result.returncode or "PASS: full App Web-home flow," not in text:
                    raise RuntimeError(f"Full App fixture failed, status={result.returncode}")
        finally:
            for server in servers:
                server.shutdown()
                server.server_close()


if __name__ == "__main__":
    main()
