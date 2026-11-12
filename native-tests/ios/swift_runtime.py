"""Inspect the actual Simulator App's Swift load commands; never rewrite binaries."""
import hashlib
import json
from pathlib import Path
import plistlib
import platform
import re
import stat
import subprocess

ROOT = Path(__file__).resolve().parents[2]
BUILD = ROOT / "src-tauri/gen/apple/build"


def require(value, message):
    if not value:
        raise RuntimeError(message)


def rpaths(output, image, arch=None):
    lines = output.splitlines()
    headers = [str(image) + ":"]
    if arch:
        headers.append(str(image) + f" (architecture {arch}):")
    require(lines and lines[0] in headers
            and [line for line in lines if line and not line[0].isspace() and line.endswith(":")] == [lines[0]]
            and sum("(architecture " in line for line in lines) <= (1 if arch else 0),
            "Expected one exact selected image in otool load commands")
    blocks = re.split(r"(?m)^Load command [0-9]+\n", output)
    require(len(blocks) > 1, "Missing actual Mach-O load commands")
    result = []
    for block in blocks[1:]:
        command = re.findall(r"(?m)^\s+cmd (LC_[A-Z0-9_]+)\s*$", block)
        require(len(command) == 1, "Ambiguous Mach-O command block")
        if command[0] == "LC_RPATH":
            paths = re.findall(r"(?m)^\s+path (.+) \(offset [0-9]+\)\s*$", block)
            require(len(paths) == 1 and paths[0].strip() == paths[0] and paths[0], "Missing actual LC_RPATH path")
            result.append(paths[0])
    return result


def require_swift_search(paths):
    require("/usr/lib/swift" in paths, "App does not search the OS Swift runtime")
    bundled = [i for i, value in enumerate(paths)
               if value in ("@executable_path/Frameworks", "@loader_path/Frameworks")]
    require(bundled and paths.index("/usr/lib/swift") < min(bundled),
            "OS Swift runtime must precede bundled back-deployment libraries")


def image_identity(path, bundle):
    require(path.is_relative_to(bundle) and path.resolve(strict=True).is_relative_to(bundle),
            "Runtime image is outside the exact App")
    current = path
    while current != bundle.parent:
        require(not current.is_symlink(), "Linked App/runtime content refused")
        current = current.parent
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode) and 0 < info.st_size <= 512 * 1024**2, "Plain bounded runtime image required")
    with path.open("rb") as stream:
        sha = hashlib.file_digest(stream, "sha256").hexdigest()
    return {"device": info.st_dev, "inode": info.st_ino, "bytes": info.st_size, "sha256": sha,
            "modified": info.st_mtime_ns}


def inspect(bundle, owner, command):
    require(not bundle.is_symlink() and bundle.resolve(strict=True).is_relative_to(owner.resolve(strict=True)),
            "Expected App inside its exact SDK build root")
    with (bundle / "Info.plist").open("rb") as source:
        info = plistlib.load(source)
    require(info.get("CFBundleIdentifier") == "com.takosumi.fixture.simulator"
            and info.get("CFBundleSupportedPlatforms") == ["iPhoneSimulator"], "Wrong Simulator App identity")
    executable = info.get("CFBundleExecutable")
    require(isinstance(executable, str) and executable and Path(executable).name == executable, "Invalid App executable")
    main = bundle / executable
    arch = platform.machine()
    require(arch in ("arm64", "x86_64"), "Unsupported Simulator SDK architecture")
    libraries = sorted((bundle / "Frameworks").glob("*.dylib"))
    require(bundle / "Frameworks/libswift_Concurrency.dylib" in libraries and len(libraries) <= 32,
            "Missing original Swift concurrency image or excessive runtime images")
    images = []
    for path in (main, *libraries):
        before = image_identity(path, bundle)
        commands = command(["xcrun", "otool", "-arch", arch, "-l", str(path)])
        dependencies = command(["xcrun", "otool", "-arch", arch, "-L", str(path)])
        dependency_lines = dependencies.splitlines()
        require(len(commands) <= 65536 and len(dependencies) <= 65536
                and len(dependency_lines) > 1 and dependency_lines[0] in (str(path) + ":", str(path) + f" (architecture {arch}):"),
                "Missing, foreign or excessive actual runtime dependencies")
        paths = rpaths(commands, path, arch)
        require(image_identity(path, bundle) == before, "Runtime image changed during SDK inspection")
        images.append({"path": str(path.relative_to(bundle)), "bytes": before["bytes"], "sha256": before["sha256"],
                       "runpaths": paths, "loadCommands": commands, "dependencies": dependencies})
    return {"scope": "Actual Simulator App Swift load-command inspection; not launch/loader closure qualification",
            "bundleIdentifier": info["CFBundleIdentifier"], "architecture": arch, "images": images}


def main():
    require(platform.system() == "Darwin", "Requires the existing macOS iOS SDK")
    apps = list(BUILD.rglob("*.app"))
    require(len(apps) == 1, "Exactly one newly built Simulator App required")
    def command(argv):
        return subprocess.check_output(argv, text=True, timeout=20)
    proof = inspect(apps[0], BUILD, command)
    proof["head"] = command(["git", "-C", str(ROOT), "rev-parse", "HEAD"]).strip()
    proof["tree"] = command(["git", "-C", str(ROOT), "rev-parse", "HEAD^{tree}"]).strip()
    path = BUILD / "swift-runtime-proof.json"
    proof["passed"] = False
    path.write_text(json.dumps(proof, indent=2) + "\n")
    # Retain actual load commands even when the search-route gate rejects.
    require_swift_search(proof["images"][0]["runpaths"])
    proof["passed"] = True
    path.write_text(json.dumps(proof, indent=2) + "\n")
    print("PASS: actual App searches OS Swift before bundled frameworks; load-command proof retained")


if __name__ == "__main__":
    main()
