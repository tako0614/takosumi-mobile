#!/usr/bin/env python3
"""Run the native GTK fixture against its own temporary loopback HTTPS server."""

import argparse
import http.server
import os
from pathlib import Path
import resource
import ssl
import socket
import subprocess
import sys
import tempfile
import threading
from urllib.parse import urlsplit
sys.dont_write_bytecode = True
from web_home import run_desktop_session


class Pages(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        page = urlsplit(self.path).path.removeprefix("/")
        if page == "failed":
            self.close_connection = True
            self.connection.shutdown(socket.SHUT_RDWR)
            self.connection.close()
            return
        if page == "slow":
            threading.Event().wait(2)
            return
        if page not in ("", "first", "second", "document/work"):
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
    # Native criticals fail the fixture, including teardown after GTK destroys
    # the widget. Keep owned subprocess crashes from leaving large core files.
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--spa-resume", action="store_true")
    parser.add_argument("--profile-reset", action="store_true")
    parser.add_argument("--failed-navigation", action="store_true")
    parser.add_argument("--cancelled-navigation", action="store_true")
    parser.add_argument("--pending-navigation", action="store_true")
    args = parser.parse_args()
    if args.profile_reset and (args.spa_resume or args.failed_navigation or args.cancelled_navigation or args.pending_navigation):
        parser.error("profile reset requires its fresh single-process fixture")
    if (args.failed_navigation or args.cancelled_navigation or args.pending_navigation) and not args.spa_resume:
        parser.error("navigation failure controls require --spa-resume")
    if sum((args.failed_navigation, args.cancelled_navigation, args.pending_navigation)) > 1:
        parser.error("select one navigation failure/pending control")
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
        env["G_DEBUG"] = ",".join(filter(None, (env.get("G_DEBUG"), "fatal-criticals")))
        for variable, name in (("XDG_DATA_HOME", "data"), ("XDG_CONFIG_HOME", "config"),
                               ("XDG_CACHE_HOME", "cache"), ("XDG_RUNTIME_DIR", "runtime")):
            directory = Path(temporary) / name
            directory.mkdir(mode=0o700)
            env[variable] = str(directory)
        try:
            if args.profile_reset:
                run_desktop_session(["xvfb-run", "-a", "dbus-run-session", "--", str(binary),
                                     "--profile-reset"], env, timeout=50)
                return
            if args.spa_resume:
                data.mkdir(mode=0o700)
                for method, extra in (("push", []), ("replace", ["--spa-replace"])):
                    controls = (["--failed-navigation"] if args.failed_navigation else []) + (
                        ["--cancelled-navigation"] if args.cancelled_navigation else [])
                    if args.pending_navigation:
                        controls.append("--pending-navigation")
                    scoped = dict(env, TAKOSUMI_FIXTURE_DATA=str(data / method))
                    for flags in ([], ["--readback"]):
                        run_desktop_session(["xvfb-run", "-a", "dbus-run-session", "--", str(binary),
                                             "--spa-resume", *extra, *controls, *flags], scoped, timeout=50)
                return
            for flags in ([], ["--readback"]):
                run_desktop_session(["xvfb-run", "-a", "dbus-run-session", "--", str(binary), *flags], env, timeout=50)
            negative = data / "missing-document.log"
            code = 0
            with negative.open("w") as output:
                try:
                    run_desktop_session(["xvfb-run", "-a", "dbus-run-session", "--", str(binary),
                                         "--readback", "--missing-document"], env, timeout=50,
                                        stdout=output, stderr=subprocess.STDOUT)
                except subprocess.CalledProcessError as error:
                    code = error.returncode
            result = negative.read_text()
            if (code != 1 or "expected fixture document not ready" not in result
                    or "observation " in result):
                raise RuntimeError(f"Wrong document was not rejected before storage access:\n{result}")
            print("PASS: missing document rejects before any storage observation", flush=True)
        finally:
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    main()
