#!/usr/bin/env python3
"""Anonymous Linux CEF candidate probe in a fresh owned desktop session."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import shutil
import subprocess
import sys
import uuid

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
sys.path.insert(0, str(HERE.parent / "desktop"))
from browser_apps import create_input
from web_home import run_desktop_session


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def main():
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--app", choices=("photopea", "excalidraw"), required=True)
    parser.add_argument("--mode", choices=("inspect", "operate", "profiles", "profile-popup", "permission-inspect", "site-permissions", "site-notifications"), default="operate")
    parser.add_argument("--notification-daemon", type=Path, help="Already available Dunst 1.12.2 executable, used only inside the owned notification fixture")
    parser.add_argument("--files", choices=("cdp", "native-picker"), default="cdp",
                        help="Native picker uses a visible browser in the owned Xvfb session")
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("This candidate has only a Linux runner")
    if args.files == "native-picker" and args.app != "excalidraw":
        parser.error("The native-picker probe currently targets Excalidraw only")
    if args.mode in ("profiles", "profile-popup", "permission-inspect", "site-permissions", "site-notifications") and args.files != "cdp":
        parser.error("The profile probe does not operate file dialogs")
    for tool in ("node", "python3", "xvfb-run", "dbus-run-session", "timeout", "convert"):
        if shutil.which(tool) is None:
            parser.error(f"Required existing capability is missing: {tool}")
    if args.files == "native-picker" or args.mode in ("permission-inspect", "site-permissions", "site-notifications"):
        for tool in ("xdotool", "import"):
            if shutil.which(tool) is None:
                parser.error(f"Required existing capability is missing: {tool}")
    if (args.mode == "site-notifications") != (args.notification_daemon is not None):
        parser.error("--notification-daemon is required only for site-notifications")
    daemon = args.notification_daemon.resolve(strict=True) if args.notification_daemon else None
    if daemon and (not daemon.is_file() or not os.access(daemon, os.X_OK)):
        parser.error("--notification-daemon must be an already available executable")
    if daemon:
        for tool in ("gdbus", "dbus-monitor"):
            if shutil.which(tool) is None:
                parser.error(f"Required existing notification capability is missing: {tool}")
    binary = args.binary.resolve(strict=True)
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error("--binary must be the already built candidate executable")
    data = args.data.absolute()
    if data.exists() or data.is_symlink():
        parser.error("--data must be a new disposable directory")
    # Resolve the existing parent before making the owned directory; no symlink
    # case/profile paths are passed to the native candidate.
    data = data.parent.resolve(strict=True) / data.name
    # This pinned Linux Chromium creates its process-singleton socket below
    # TMPDIR. sockaddr_un reserves one byte for NUL in its 108-byte path.
    socket_path = data / "tmp/org.chromium.Chromium.XXXXXX/SingletonSocket"
    if len(os.fsencode(socket_path)) >= 108:
        parser.error("--data is too long for the Linux Chromium socket; choose a shorter owned case path")
    data.mkdir(mode=0o700)
    nonce = str(uuid.uuid4())
    (data / ".owned-browser-reference").write_text(nonce)
    fixture = create_input(data, args.app, nonce)
    sources = [HERE / name for name in ("src/main.rs", "Cargo.toml", "Cargo.lock", "operate.mjs", "gtk-readiness.mjs", "profiles.mjs", "permissions.mjs", "notification-backend.mjs", "notifications.mjs", "x11-owner.py", "run.py")]
    sources += [HERE.parent / name for name in (
        "desktop/browser_apps.py", "desktop/web_home.py", "browser/guard.py", "browser/lifecycle.ts",
    )]
    # The native candidate compiles the product's request validator library.
    sources += [REPO / "src-tauri/plugins/app-screen" / name for name in
                ("Cargo.toml", "build.rs", "src/lib.rs", "src/models.rs", "src/error.rs")]
    hashes = {str(path.relative_to(REPO)): digest(path) for path in sources}
    snapshots = data / "source-snapshot"
    for path in sources:
        destination = snapshots / path.relative_to(REPO)
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, destination)
    library = binary.parent / "libcef.so"
    library_hash = digest(library)
    source = {
        "app": args.app, "mode": args.mode, "files": args.files, "binarySha256": digest(binary),
        "candidateMainSha256": digest(HERE / "src/main.rs"),
        "driverSha256": digest(HERE / "operate.mjs"),
        "runnerSha256": digest(Path(__file__).resolve()),
        "inputSha256": digest(fixture),
        "lockSha256": digest(HERE / "Cargo.lock"),
        "sources": hashes, "runtimeLibrarySha256": library_hash,
        "notificationDaemonSha256": digest(daemon) if daemon else None,
    }
    (data / "source.json").write_text(json.dumps(source, indent=2) + "\n")
    # No inherited token, browser profile, keyring or operator configuration.
    env = {key: value for key, value in os.environ.items()
           if key in ("PATH", "USER", "LOGNAME", "LANG", "LC_ALL", "LC_CTYPE")}
    for key, name in (("HOME", "session-home"), ("TMPDIR", "session-tmp"),
                      ("XDG_DATA_HOME", "session-data"), ("XDG_CONFIG_HOME", "session-config"),
                      ("XDG_CACHE_HOME", "session-cache"), ("XDG_RUNTIME_DIR", "session-runtime")):
        directory = data / name
        directory.mkdir(mode=0o700)
        env[key] = str(directory)
    env.update(PYTHONDONTWRITEBYTECODE="1", TAKOSUMI_CEF_RUNNER_CASE=str(data))
    print(f"CASE {data}", flush=True)
    result = {"exitCode": None, "error": None, "binaryUnchanged": False,
              "sourcesUnchanged": False, "runtimeUnchanged": False, "daemonUnchanged": False, "passed": False}
    try:
        with (data / "session.log").open("x") as output:
            run_desktop_session([
                "xvfb-run", "-a", "dbus-run-session", "--", "timeout", "150",
                "node", str(HERE / "operate.mjs"), str(data), args.app, args.mode, str(binary), args.files,
                *([str(daemon)] if daemon else []),
            ], env, timeout=170, stdout=output, stderr=subprocess.STDOUT)
        result["exitCode"] = 0
    except subprocess.CalledProcessError as error:
        result.update(exitCode=error.returncode, error=str(error))
    except Exception as error:
        result["error"] = str(error)
    finally:
        result["binaryUnchanged"] = digest(binary) == source["binarySha256"]
        result["sourcesUnchanged"] = all(path.is_file() and digest(path) == hashes[str(path.relative_to(REPO))]
                                         for path in sources)
        result["runtimeUnchanged"] = digest(library) == library_hash
        result["daemonUnchanged"] = not daemon or digest(daemon) == source["notificationDaemonSha256"]
        result["passed"] = (result["exitCode"] == 0 and not result["error"]
                            and result["binaryUnchanged"] and result["sourcesUnchanged"]
                            and result["runtimeUnchanged"] and result["daemonUnchanged"])
        (data / "execution.json").write_text(json.dumps(result, indent=2) + "\n")
        print((data / "session.log").read_text(), end="", flush=True)
    if not result["passed"]:
        raise SystemExit(f"Native probe failed; retain evidence at {data}")


if __name__ == "__main__":
    main()
