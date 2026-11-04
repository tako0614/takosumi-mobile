#!/usr/bin/env python3
"""Run the native GTK fixture against its own temporary loopback HTTPS server."""

import argparse
import http.server
import os
from pathlib import Path
import ssl
import subprocess
import tempfile
import threading


class Pages(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        page = self.path.removeprefix("/")
        if page not in ("", "first", "second"):
            self.send_error(404)
            return
        body = ("<!doctype html><meta name='sumi-fixture' content='bootstrap'>"
                f"<title>Fixture</title><p>{page}</p>").encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

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
    with tempfile.TemporaryDirectory(prefix="sumi-fixture-tls-", dir=data.parent) as temporary:
        cert = Path(temporary) / "cert.pem"
        key = Path(temporary) / "key.pem"
        subprocess.run([
            "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
            "-keyout", str(key), "-out", str(cert), "-days", "1",
            "-subj", "/CN=127.0.0.1", "-addext", "subjectAltName=IP:127.0.0.1",
        ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Pages)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        server.socket = context.wrap_socket(server.socket, server_side=True)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        env = dict(os.environ, TAKOSUMI_FIXTURE_DATA=str(data),
                   TAKOSUMI_FIXTURE_HTTPS=f"https://127.0.0.1:{server.server_port}",
                   TAKOSUMI_FIXTURE_CERT=str(cert))
        try:
            for flags in ([], ["--readback"]):
                subprocess.run(["xvfb-run", "-a", "dbus-run-session", "--", str(binary), *flags],
                               env=env, check=True)
            rejected = subprocess.run([
                "xvfb-run", "-a", "dbus-run-session", "--", str(binary),
                "--readback", "--missing-document",
            ], env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                timeout=50)
            (data / "missing-document.log").write_text(rejected.stdout)
            if (rejected.returncode != 1 or "expected fixture document not ready" not in rejected.stdout
                    or "observation " in rejected.stdout):
                raise RuntimeError(f"Wrong document was not rejected before storage access:\n{rejected.stdout}")
            print("PASS: missing document rejects before any storage observation", flush=True)
        finally:
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    main()
