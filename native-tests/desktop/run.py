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
import time
from urllib.parse import parse_qs, urlsplit
sys.dont_write_bytecode = True
from web_home import enable_child_subreaper, observe_children, run_desktop_session, wait_engine_children


class Pages(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        request = urlsplit(self.path)
        page = request.path.removeprefix("/")
        recovery_ready = False
        if page == "retry" or (page == "slow" and "case" in parse_qs(request.query)):
            method = parse_qs(request.query).get("case", [""])[0]
            if method not in ("push", "replace"):
                self.send_error(400)
                return
            # Test-only server recovery, armed after the stock error finished.
            # It never writes the product's resume metadata or supplies a route.
            recovery_ready = (self.server.fixture_data / method / "native-recovery-ready").is_file()
        if page == "failed" or (page == "retry" and not recovery_ready):
            self.close_connection = True
            self.connection.shutdown(socket.SHUT_RDWR)
            self.connection.close()
            return
        if page == "slow" and not recovery_ready:
            threading.Event().wait(2)
            return
        if page not in ("", "first", "second", "document/work", "document/after-error", "retry", "slow"):
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


def run_native_writer(command, env, path, markers):
    # A stale binary that ignores a new flag must not qualify an empty case.
    with path.open("w+") as output:
        try:
            run_desktop_session(command, env, timeout=50, stdout=output, stderr=subprocess.STDOUT)
        finally:
            output.seek(0)
            captured = output.read()
            print(captured, end="", flush=True)
    for marker in markers:
        if marker not in captured.splitlines():
            raise RuntimeError(f"Native writer did not report: {marker}")


def run_native_generation(command):
    # This worker lives inside the owned Xvfb/D-Bus session. Preserve that
    # session until the native generation has shut down, including its helpers.
    enable_child_subreaper()
    tracked, descriptions = {}, {}
    child = subprocess.Popen(command)
    deadline = time.monotonic() + 45
    try:
        while child.poll() is None:
            observe_children(child.pid, tracked, descriptions)
            if time.monotonic() >= deadline:
                raise TimeoutError("Native fixture process did not exit")
            time.sleep(0.025)
    finally:
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=5)
    wait_engine_children(tracked, descriptions)
    print(f"Native fixture {child.pid} and {len(tracked)} observed engine children exited", flush=True)
    print(f"PASS: native generation shutdown completed, fixture status={child.returncode}", flush=True)
    return child.returncode


def native_command(binary, flags):
    return ["xvfb-run", "-a", "dbus-run-session", "--", sys.executable, "-B",
            str(Path(__file__).resolve()), "--native-worker", str(binary), *flags]


def main():
    # Native criticals fail the fixture, including teardown after GTK destroys
    # the widget. Keep owned subprocess crashes from leaving large core files.
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    if len(sys.argv) > 1 and sys.argv[1] == "--native-worker":
        if not os.environ.get("DISPLAY") or not os.environ.get("TAKOSUMI_FIXTURE_DATA"):
            raise RuntimeError("Native worker requires the isolated outer runner")
        sys.exit(run_native_generation(sys.argv[2:]))
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--spa-resume", action="store_true")
    parser.add_argument("--failed-navigation", action="store_true")
    parser.add_argument("--error-page-close", action="store_true")
    parser.add_argument("--error-page-back", action="store_true")
    parser.add_argument("--error-page-retry", action="store_true")
    parser.add_argument("--cancelled-navigation", action="store_true")
    parser.add_argument("--cancelled-spa", action="store_true")
    parser.add_argument("--pending-navigation", action="store_true")
    args = parser.parse_args()
    if args.error_page_close and not args.failed_navigation:
        parser.error("error-page close requires --failed-navigation")
    if (args.error_page_back or args.error_page_retry) and not args.error_page_close:
        parser.error("error-page recovery requires --error-page-close")
    if args.error_page_back and args.error_page_retry:
        parser.error("select one error-page recovery action")
    if args.cancelled_spa and not args.cancelled_navigation:
        parser.error("cancelled SPA recovery requires --cancelled-navigation")
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
        server.fixture_data = data
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
            if args.spa_resume:
                data.mkdir(mode=0o700)
                for method, extra in (("push", []), ("replace", ["--spa-replace"])):
                    controls = (["--failed-navigation"] if args.failed_navigation else []) + (
                        ["--cancelled-navigation"] if args.cancelled_navigation else [])
                    if args.error_page_close:
                        controls.append("--error-page-close")
                    if args.error_page_back:
                        controls.append("--error-page-back")
                    if args.error_page_retry:
                        controls.append("--error-page-retry")
                    if args.cancelled_spa:
                        controls.append("--cancelled-spa")
                    if args.pending_navigation:
                        controls.append("--pending-navigation")
                    scoped = dict(env, TAKOSUMI_FIXTURE_DATA=str(data / method))
                    for flags in ([], ["--readback"]):
                        command = native_command(binary, ["--spa-resume", *extra, *controls, *flags])
                        if (args.error_page_close or args.cancelled_spa) and not flags:
                            markers = ["PASS: stock error document committed and finished before native Close"] if args.error_page_close else []
                            if args.error_page_back:
                                markers.extend(["PASS: native Back restored a cached document before a later SPA change",
                                                "PASS: cached Back accepts the next same-document route"])
                            if args.error_page_retry:
                                markers.append("PASS: native Reload completed at the previously failed URL")
                            if args.cancelled_spa:
                                markers.append("PASS: surviving document changed to the previously cancelled URL")
                            run_native_writer(command, scoped, Path(temporary) / f"{method}-recovery.log", markers)
                        else:
                            run_desktop_session(command, scoped, timeout=50)
                return
            for flags in ([], ["--readback"]):
                run_desktop_session(native_command(binary, flags), env, timeout=50)
            negative = data / "missing-document.log"
            code = 0
            with negative.open("w") as output:
                try:
                    run_desktop_session(native_command(binary, ["--readback", "--missing-document"]), env, timeout=50,
                                        stdout=output, stderr=subprocess.STDOUT)
                except subprocess.CalledProcessError as error:
                    code = error.returncode
            result = negative.read_text()
            if (code != 1 or "expected fixture document not ready" not in result
                    or "observation " in result
                    or "PASS: native generation shutdown completed, fixture status=1" not in result.splitlines()):
                raise RuntimeError(f"Wrong document was not rejected before storage access:\n{result}")
            print("PASS: missing document rejects before any storage observation", flush=True)
        finally:
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    main()
