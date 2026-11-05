#!/usr/bin/env python3
"""Run the full bundled App against disposable mock Web login/home/app sites."""

import argparse
import ctypes
import http.server
from http.cookies import SimpleCookie
import os
from pathlib import Path
import resource
import signal
import ssl
import subprocess
import tempfile
import threading
import time
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


def process_identity(pid):
    try:
        stat = Path(f"/proc/{pid}/stat").read_text().split(") ", 1)[1].split()
        return stat[19], stat[0]  # start time prevents mistaking a reused PID
    except (FileNotFoundError, ProcessLookupError):
        return None


def descendants(pid):
    try:
        tasks = list(Path(f"/proc/{pid}/task").iterdir())
    except (FileNotFoundError, ProcessLookupError):
        return set()
    found = set()
    for task in tasks:
        try:
            found.update(map(int, (task / "children").read_text().split()))
        except (FileNotFoundError, ProcessLookupError):
            pass
    for child in list(found):
        found.update(descendants(child))
    return found


def process_description(pid):
    details = {"pid": pid, "identity": process_identity(pid)}
    try:
        details["name"] = Path(f"/proc/{pid}/comm").read_text().strip()
        details["executable"] = Path(os.readlink(f"/proc/{pid}/exe")).name
    except (FileNotFoundError, ProcessLookupError, PermissionError):
        pass
    return details


def enable_child_subreaper():
    # Only the dedicated session worker calls this. It adopts that worker's
    # orphaned App descendants so a failed NetworkProcess cannot look like a
    # successful shutdown merely because its PID disappeared.
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(36, 1, 0, 0, 0):  # PR_SET_CHILD_SUBREAPER
        error = ctypes.get_errno()
        raise OSError(error, os.strerror(error))


def observe_children(parent, tracked, descriptions):
    for pid in descendants(parent):
        identity = process_identity(pid)
        if not identity:
            continue
        if tracked.get(pid) != identity[0]:
            descriptions[pid] = process_description(pid)
        elif not is_network_process(descriptions[pid]):
            # A launcher may exec/rename after its first observation without
            # changing start time. Never lose a confirmed network role.
            descriptions[pid] = process_description(pid)
        tracked[pid] = identity[0]


def is_network_process(description):
    return (description.get("executable") == "WebKitNetworkProcess" or
            description.get("name") == "WebKitNetworkPr")


def reap_observed_children(tracked, descriptions, statuses):
    for pid, start in tracked.items():
        identity = process_identity(pid)
        if not identity or identity[0] != start or identity[1] != "Z":
            continue
        # A zombie cannot exec/rename between this role check and waitpid.
        if not is_network_process(descriptions[pid]):
            descriptions[pid] = process_description(pid)
        try:
            waited, status = os.waitpid(pid, os.WNOHANG)
        except ChildProcessError:
            continue  # Still owned/reaped by an intermediate App descendant.
        if waited:
            statuses[pid] = (start, os.waitstatus_to_exitcode(status))


def shutdown_description(pid):
    details = process_description(pid)
    threads = []
    try:
        for task in sorted(Path(f"/proc/{pid}/task").iterdir())[:12]:
            try:
                threads.append({"tid": int(task.name), "name": (task / "comm").read_text().strip(),
                                "wait": (task / "wchan").read_text().strip()})
            except (FileNotFoundError, ProcessLookupError, PermissionError):
                pass
    except (FileNotFoundError, ProcessLookupError, PermissionError):
        pass
    details["threads"] = threads
    return details


def run_apps(binary, data, quick_close, legacy_entry):
    """Both Apps restart inside one disposable desktop session, as in use."""
    previous_pid = None
    for readback in (False, True):
        flags = ["--readback"] if readback else []
        if quick_close:
            flags.append("--quick-close")
        if legacy_entry:
            flags.append("--legacy-entry")
        log = data / ("readback.log" if readback else "first.log")
        tracked = {}
        descriptions = {}
        statuses = {}
        with log.open("w") as output:
            child = subprocess.Popen([str(binary), *flags], stdout=output, stderr=subprocess.STDOUT)
            assert child.pid != previous_pid, "App restart must use a new process"
            previous_pid = child.pid
            deadline = time.monotonic() + 75
            try:
                while child.poll() is None:
                    observe_children(child.pid, tracked, descriptions)
                    if time.monotonic() >= deadline:
                        raise TimeoutError("App process did not exit")
                    time.sleep(0.025)
            finally:
                if child.poll() is None:
                    child.terminate()
                    try:
                        child.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait(timeout=5)
        text = log.read_text()
        print(text, end="", flush=True)
        if child.returncode or "PASS: full App Web-home flow," not in text:
            raise RuntimeError(f"Full App fixture failed, status={child.returncode}")
        assert tracked, "Expected to observe native engine child processes"
        # Keep Xvfb/D-Bus alive until the previous generation actually exits.
        # This waits for process lifecycle only, never storage values or success.
        deadline = time.monotonic() + 5
        while True:
            # After the App exits, all remaining descendants of this dedicated
            # worker belong to that App generation, including a helper forked
            # between the final observation and App exit.
            observe_children(os.getpid(), tracked, descriptions)
            reap_observed_children(tracked, descriptions, statuses)
            failed_network = [dict(descriptions[pid], status=status) for pid, (start, status) in statuses.items()
                              if tracked[pid] == start and status != 0 and
                              is_network_process(descriptions[pid])]
            if failed_network:
                raise RuntimeError(f"App NetworkProcess exited abnormally: {failed_network}")
            alive = [pid for pid, start in tracked.items()
                     if (identity := process_identity(pid)) and identity[0] == start
                     and (identity[1] != "Z" or statuses.get(pid, (None,))[0] != start)]
            if not alive:
                break
            if time.monotonic() >= deadline:
                raise RuntimeError(f"Old App engine children still alive: {[shutdown_description(pid) for pid in alive]}")
            time.sleep(0.025)
        # WebKit may deliberately kill a WebProcess. Record its status without
        # treating that rendering-process policy as a NetworkProcess flush.
        print(f"Observed orphaned App child exit codes: {statuses}", flush=True)
        unknown_network = [pid for pid, start in tracked.items()
                           if is_network_process(descriptions[pid]) and statuses.get(pid, (None,))[0] != start]
        if unknown_network:
            print(f"Unknown NetworkProcess exit status (not available to worker): {unknown_network}", flush=True)
        print(f"PASS: App {child.pid} and {len(tracked)} observed engine children exited, readback={readback}", flush=True)


def run_desktop_session(command, env, timeout=170, *, stdout=None, stderr=None):
    # All session tools belong to this new process group. A timeout must clean
    # the owned session, not just kill xvfb-run and leave its children behind.
    process = subprocess.Popen(command, env=env, start_new_session=True, stdout=stdout, stderr=stderr)
    try:
        result = process.wait(timeout=timeout)
        if result:
            raise subprocess.CalledProcessError(result, command)
    finally:
        for sig in (signal.SIGTERM, signal.SIGKILL):
            try:
                os.killpg(process.pid, sig)
            except ProcessLookupError:
                pass
            if sig == signal.SIGTERM:
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    pass
        process.wait(timeout=5)


def main():
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--quick-close", action="store_true")
    parser.add_argument("--legacy-entry", action="store_true")
    parser.add_argument("--session-worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    data = args.data.absolute()
    if args.session_worker:
        if not data.is_dir() or not os.environ.get("DISPLAY"):
            parser.error("session worker requires the isolated outer runner")
        enable_child_subreaper()
        run_apps(binary, data, args.quick_close, args.legacy_entry)
        return
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
                   TAKOSUMI_FIXTURE_HTTPS=f"https://127.0.0.1:{servers[1].server_port}",
                   TAKOSUMI_FIXTURE_APP_HTTPS=servers[1].app_origin)
        env["G_DEBUG"] = ",".join(filter(None, (env.get("G_DEBUG"), "fatal-criticals")))
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
            flags = ["--quick-close"] if args.quick_close else []
            if args.legacy_entry:
                flags.append("--legacy-entry")
            run_desktop_session([
                "xvfb-run", "-a", "dbus-run-session", "--", "python3", str(Path(__file__).resolve()),
                "--session-worker", "--binary", str(binary), "--data", str(data), *flags,
            ], env)
        finally:
            for server in servers:
                server.shutdown()
                server.server_close()


if __name__ == "__main__":
    main()
