#!/usr/bin/env python3
"""Public Web-app operations in owned GTK profiles; never signs in or saves to cloud."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import shutil
import struct
import subprocess
import sys
import time
from urllib.parse import unquote, urlparse
import uuid

from web_home import (
    enable_child_subreaper, observe_children, run_desktop_session, wait_engine_children,
)

ORIGINS = {"photopea": "https://www.photopea.com", "excalidraw": "https://excalidraw.com"}
PROFILES = {"work": "11111111-1111-4111-8111-111111111111",
            "personal": "22222222-2222-4222-8222-222222222222"}


def create_input(data, app, nonce):
    if app == "photopea":
        path = data / "SumiProbeWork.svg"
        path.write_text(
            '<svg xmlns="http://www.w3.org/2000/svg" width="128" height="96">'
            f'<!-- owned fixture {nonce} -->'
            '<rect width="64" height="96" fill="#ff0000"/>'
            '<rect x="64" width="64" height="96" fill="#0000ff"/></svg>')
    else:
        path = data / "SumiProbeWork.excalidraw"
        path.write_text(json.dumps({
            "type": "excalidraw", "version": 2, "source": "owned-browser-probe",
            "elements": [{
                "id": nonce, "type": "rectangle", "x": 80, "y": 80, "width": 128, "height": 96,
                "angle": 0, "strokeColor": "#000000", "backgroundColor": "#ff0000",
                "fillStyle": "solid", "strokeWidth": 1, "strokeStyle": "solid", "roughness": 0,
                "opacity": 100, "groupIds": [], "frameId": None, "roundness": None,
                "seed": 1, "version": 1, "versionNonce": 1, "isDeleted": False,
                "boundElements": None, "updated": 1, "link": None, "locked": False,
            }],
            "appState": {"viewBackgroundColor": "#ffffff", "name": "SumiProbeWork"}, "files": {},
        }))
    return path


def validate_report(report, data, app, mode, nonce):
    if not isinstance(report, dict) or report.get("error"):
        raise RuntimeError(f"Native operation did not complete: {report.get('error') if isinstance(report, dict) else report}")
    for key, expected in (("app", app), ("mode", mode), ("nonce", nonce),
                          ("origin", ORIGINS[app]), ("profileIsolation", True),
                          ("operationRequested", mode == "operate")):
        if report.get(key) != expected:
            raise RuntimeError(f"Native receipt mismatch: {key}")
    for field in ("context", "personalContext"):
        context = report.get(field, {})
        if (context.get("origin") != ORIGINS[app] or context.get("secure") is not True
                or any(context.get(key) != "undefined" for key in ("tauri", "ipc", "wryHandler"))):
            raise RuntimeError("Actual site/security/remote-IPC context did not match")
    for name, profile in PROFILES.items():
        for kind in ("data", "cache"):
            actual = Path(report.get("profiles", {}).get(name, {}).get(kind, ""))
            expected = (data / "profiles" / profile).resolve()
            if not actual.is_absolute() or not actual.resolve().is_relative_to(expected):
                raise RuntimeError(f"The {name} {kind} path is not its original native profile")
    if mode == "observe":
        return None
    downloads = report.get("downloads")
    if not isinstance(downloads, list) or len(downloads) != 1:
        raise RuntimeError("Exactly one actual completed download receipt is required")
    download = downloads[0]
    if download.get("finished") is not True or download.get("failed") is not False:
        raise RuntimeError("The native download did not finish successfully")
    raw = download.get("destination")
    if not isinstance(raw, str):
        raise RuntimeError("The actual download has no destination")
    parsed = urlparse(raw)
    if parsed.query or parsed.fragment:
        raise RuntimeError("Native file destination contains unexpected URL metadata")
    if parsed.scheme and parsed.scheme != "file":
        raise RuntimeError("Native download destination is not a local file")
    if parsed.scheme and parsed.netloc not in ("", "localhost"):
        raise RuntimeError("Native download is not in the owned local session")
    path = Path(unquote(parsed.path) if parsed.scheme else raw)
    if not path.is_absolute() or path.is_symlink() or not path.resolve().is_relative_to((data / "Downloads").resolve()):
        raise RuntimeError("The actual download escaped the owned case")
    if not path.is_file() or not 0 < path.stat().st_size <= 1024 * 1024:
        raise RuntimeError("The actual export is missing, empty or oversized")
    return path


def verify_export(path, app, nonce):
    raw = path.read_bytes()  # after native finish; incorrect data is never retried
    if app == "excalidraw":
        scene = json.loads(raw)
        items = [item for item in scene.get("elements", [])
                 if item.get("id") == nonce and not item.get("isDeleted")]
        if len(items) != 1:
            raise RuntimeError("The exported scene did not preserve the imported rectangle")
        item = items[0]
        if item.get("type") != "rectangle" or item.get("y") != 80 or item.get("width") != 128 or item.get("height") != 96:
            raise RuntimeError("The exported scene changed unrelated geometry")
        if not 80 < item.get("x", 0) <= 100:
            raise RuntimeError("The exported scene does not contain the keyboard edit")
    else:
        if raw[:8] != b"\x89PNG\r\n\x1a\n" or len(raw) < 33 or raw[12:16] != b"IHDR":
            raise RuntimeError("Photopea did not export an actual PNG")
        if struct.unpack(">II", raw[16:24]) != (128, 96):
            raise RuntimeError("Photopea did not export the fixture's image dimensions")
        # Use the already installed PNG decoder, with bounded input and output.
        pixels = subprocess.run([
            "convert", "-limit", "memory", "32MiB", "-limit", "map", "32MiB", "-limit", "disk", "0",
            "PNG:" + str(path), "-alpha", "on", "-depth", "8", "RGBA:-",
        ], check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=3).stdout
        if len(pixels) != 128 * 96 * 4:
            raise RuntimeError("PNG decoder returned an unexpected image")
        left = pixels[(48 * 128 + 16) * 4:][:4]
        right = pixels[(48 * 128 + 112) * 4:][:4]
        if not (left[0] > 240 and max(left[1:3]) < 15 and left[3] > 240
                and right[2] > 240 and max(right[:2]) < 15 and right[3] > 240):
            raise RuntimeError("The exported PNG does not contain the selected fixture's pixels")
    return {"path": str(path), "bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}


def run_worker(binary, data, app, mode, nonce):
    enable_child_subreaper()
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    tracked, descriptions = {}, {}
    before = hashlib.sha256(binary.read_bytes()).hexdigest()
    (data / "execution.json").write_text(json.dumps({
        "app": app, "mode": mode, "binary": str(binary), "binarySha256": before,
        "runnerSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
    }, indent=2) + "\n")
    with (data / "native.log").open("w") as output:
        child = subprocess.Popen([str(binary)], env=os.environ, cwd=data / "Downloads",
                                 stdout=output, stderr=subprocess.STDOUT)
        deadline = time.monotonic() + 160
        try:
            while child.poll() is None:
                observe_children(child.pid, tracked, descriptions)
                if time.monotonic() >= deadline:
                    raise TimeoutError("Native browser operation did not exit")
                time.sleep(0.025)
        finally:
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=5)
    # Retain the desktop session until its engine children have stopped.
    if not tracked:
        raise RuntimeError("No native engine child was observed")
    wait_engine_children(tracked, descriptions)
    report_path = data / "native-report.json"
    if child.returncode or not report_path.is_file():
        print((data / "native.log").read_text()[-5000:], flush=True)
        raise RuntimeError(f"Native operation failed, status={child.returncode}; see {data}")
    report = json.loads(report_path.read_text())
    destination = validate_report(report, data, app, mode, nonce)
    export = verify_export(destination, app, nonce) if destination else None
    if hashlib.sha256(binary.read_bytes()).hexdigest() != before:
        raise RuntimeError("The native binary changed during the operation")
    receipt = {"app": app, "mode": mode, "binarySha256": before,
               "operationPassed": mode == "operate", "export": export, "native": report}
    (data / "result.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(("PASS: actual app file operation " if export else "OBSERVATION ONLY: no app operation qualification ")
          + app + " -> " + str(data), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--app", choices=tuple(ORIGINS), required=True)
    parser.add_argument("--mode", choices=("operate", "observe"), default="operate")
    parser.add_argument("--session-worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    data = args.data.absolute()
    if args.session_worker:
        nonce = os.environ.get("TAKOSUMI_BROWSER_NONCE")
        if not nonce or not os.environ.get("DISPLAY") or os.environ.get("TAKOSUMI_BROWSER_DATA") != str(data):
            parser.error("Worker requires its owned outer Xvfb session")
        if (data / ".owned-browser-probe").read_text() != nonce:
            parser.error("Worker ownership marker mismatch")
        run_worker(binary, data, args.app, args.mode, nonce)
        return
    if data.exists() or data.is_symlink():
        parser.error("--data must be a new disposable directory")
    for tool in ("xvfb-run", "dbus-run-session", "xdotool", "timeout", "import", "convert"):
        if shutil.which(tool) is None:
            parser.error(f"Required existing capability is missing: {tool}")
    data.mkdir(parents=True, mode=0o700)
    nonce = str(uuid.uuid4())
    (data / ".owned-browser-probe").write_text(nonce)
    input_path = create_input(data, args.app, nonce)
    env = dict(os.environ, TAKOSUMI_BROWSER_DATA=str(data), TAKOSUMI_BROWSER_APP=args.app,
               TAKOSUMI_BROWSER_MODE=args.mode, TAKOSUMI_BROWSER_NONCE=nonce)
    for key, name in (("XDG_DATA_HOME", "data"), ("XDG_CONFIG_HOME", "config"),
                      ("XDG_CACHE_HOME", "cache"), ("XDG_RUNTIME_DIR", "runtime")):
        directory = data / name
        directory.mkdir(mode=0o700)
        env[key] = str(directory)
    (data / "Downloads").mkdir(mode=0o700)
    # GLib supplies WebKit's default Downloads path. Native startup verifies it
    # before opening a public page; no custom destination callback is installed.
    (data / "config" / "user-dirs.dirs").write_text(f'XDG_DOWNLOAD_DIR="{data / "Downloads"}"\n')
    (data / "input-receipt.json").write_text(json.dumps({
        "path": str(input_path), "sha256": hashlib.sha256(input_path.read_bytes()).hexdigest(),
    }, indent=2) + "\n")
    env["G_DEBUG"] = ",".join(filter(None, (env.get("G_DEBUG"), "fatal-criticals")))
    with (data / "session.log").open("w") as output:
        run_desktop_session([
            "xvfb-run", "-a", "dbus-run-session", "--", "python3", "-B", str(Path(__file__).resolve()),
            "--session-worker", "--binary", str(binary), "--data", str(data), "--app", args.app,
            "--mode", args.mode,
        ], env, timeout=190, stdout=output, stderr=subprocess.STDOUT)
    print((data / "session.log").read_text(), end="")


if __name__ == "__main__":
    main()
