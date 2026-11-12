#!/usr/bin/env bash
# Compile the whole Tauri App for an x86_64 emulator. No install or publication.
set -euo pipefail

for signing_input in ANDROID_KEYSTORE ANDROID_KEYSTORE_PATH ANDROID_KEYSTORE_PASSWORD \
  ANDROID_KEY_ALIAS ANDROID_KEY_PASSWORD TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD; do
  if printenv "$signing_input" > /dev/null; then
    echo "Refusing signing input: $signing_input" >&2
    exit 2
  fi
done
if [[ "$(uname -s)" != Linux || "$(uname -m)" != x86_64 ]]; then
  echo 'This check requires a Linux x86_64 Android SDK host.' >&2
  exit 2
fi
cd "$(dirname "$0")/../.."
if [[ -e src-tauri/gen/android || -L src-tauri/gen/android ]]; then
  echo 'Use a disposable checkout without an existing generated Android project.' >&2
  exit 2
fi
if [[ -n "${CARGO_TARGET_DIR:-}" && "$CARGO_TARGET_DIR" != "$PWD/src-tauri/target" ]]; then
  echo 'Refusing a foreign Cargo build target.' >&2
  exit 2
fi
export CARGO_TARGET_DIR="$PWD/src-tauri/target"
: "${ANDROID_HOME:?An installed Android SDK is required}"
: "${NDK_HOME:?An installed Android NDK is required}"
test -f "$ANDROID_HOME/platforms/android-36/android.jar"
test -x "$ANDROID_HOME/build-tools/36.0.0/apksigner"
test -x "$ANDROID_HOME/build-tools/36.0.0/aapt2"
test -x "$ANDROID_HOME/cmdline-tools/latest/bin/apkanalyzer"
test -x "$NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/x86_64-linux-android28-clang"
cat "$NDK_HOME/source.properties"
java -version
rustup target add x86_64-linux-android

# Same product plugins, manifest hardening and embedded shared UI. The private
# package ID is used consistently in generation and build. Gradle supplies its
# disposable debug signature; no production signing key or Firebase config.
fixture_config='{"identifier":"com.takosumi.fixture.android"}'
bun ../mobile-kit/scripts/init-tauri-mobile-native.mjs --platform android \
  -- --ci --skip-targets-install --config "$fixture_config"
bunx tauri android build --ci --debug --target x86_64 --apk \
  --config "$fixture_config" -- --locked

python3 -B - <<'PY'
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import stat
import struct
import subprocess
import sys
import xml.etree.ElementTree as ET
import zipfile

def require(value, message):
    if not value:
        raise RuntimeError(message)

def digest(stream):
    return hashlib.file_digest(stream, "sha256").hexdigest()

root = Path.cwd()
sys.path.insert(0, str(root / "native-tests/android"))
from apk_policy import extraction_resource
generated = root / "src-tauri/gen/android"
apks = list((generated / "app/build/outputs/apk").rglob("*.apk"))
require(len(apks) == 1, "Expected exactly one newly built debug App APK")
require(not list(generated.rglob("*.aab")), "Unexpected App bundle")
require(not list(generated.rglob("google-services.json")), "Unexpected Firebase configuration")
apk = apks[0]
require(apk.is_file() and not apk.is_symlink() and apk.stat().st_size > 0, "Missing plain APK")
sdk = Path(os.environ["ANDROID_HOME"])
subprocess.run([str(sdk / "build-tools/36.0.0/apksigner"), "verify", "--verbose", str(apk)],
               check=True, timeout=30)
manifest_text = subprocess.check_output(
    [str(sdk / "cmdline-tools/latest/bin/apkanalyzer"), "manifest", "print", str(apk)],
    text=True, timeout=30)
manifest = ET.fromstring(manifest_text)
android = "{http://schemas.android.com/apk/res/android}"
require(manifest.get("package") == "com.takosumi.fixture.android", "Wrong APK package")
sdk_node = manifest.find("uses-sdk")
require(sdk_node is not None and sdk_node.get(android + "minSdkVersion") == "28"
        and sdk_node.get(android + "targetSdkVersion") == "36", "Wrong actual APK SDK contract")
app = manifest.find("application")
require(app is not None and app.get(android + "debuggable") == "true", "Expected a debug App")
require(app.get(android + "allowBackup") == "false", "Lost product backup hardening")
resource_table = subprocess.check_output(
    [str(sdk / "build-tools/36.0.0/aapt2"), "dump", "resources", str(apk)], text=True, timeout=30)
rules_file = extraction_resource(app.get(android + "dataExtractionRules"), resource_table)
rules_text = subprocess.check_output(
    [str(sdk / "cmdline-tools/latest/bin/apkanalyzer"), "resources", "xml", "--file",
     "/" + rules_file, str(apk)], text=True, timeout=30)
rules = ET.fromstring(rules_text)
expected_rules = ET.parse(generated / "app/src/main/res/xml/data_extraction_rules.xml").getroot()
def policy(node):
    return [node.tag, sorted(node.attrib.items()), [policy(child) for child in node]]
require(rules.tag == "data-extraction-rules" and policy(rules) == policy(expected_rules),
        "Actual APK extraction rules differ from the normal Kit generated input")
for section in ("cloud-backup", "device-transfer"):
    nodes = rules.findall(section)
    require(len(nodes) == 1 and nodes[0].findall("exclude"), "Missing secure-store exclusions")
services = [s for s in app.findall("service")
            if s.get(android + "name") == "com.takosumi.push.TakosumiMessagingService"]
require(len(services) == 1 and services[0].get(android + "exported") == "false",
        "Lost normal nonexported push service")
require(any(a.get(android + "name") == "com.google.firebase.MESSAGING_EVENT"
            for a in services[0].findall("intent-filter/action")), "Lost normal push action")
metadata = {n.get(android + "name"): n.get(android + "value") for n in app.findall("meta-data")}
require(metadata.get("firebase_messaging_auto_init_enabled") == "false"
        and metadata.get("firebase_analytics_collection_enabled") == "false",
        "Lost opt-in Firebase initialization policy")
schemes = [n.get(android + "scheme") for n in app.findall("activity/intent-filter/data")]
require("takosumi" in schemes, "Lost product deep-link scheme")
target = Path(os.environ.get("CARGO_TARGET_DIR", root / "src-tauri/target"))
native = target / "x86_64-linux-android/debug/libtakosumi_mobile_lib.so"
require(native.is_file() and not native.is_symlink(), "Missing actual Cargo App library")
with native.open("rb") as stream:
    native_hash = digest(stream)
with zipfile.ZipFile(apk) as archive:
    entries = archive.infolist()
    names = [e.filename for e in entries]
    require(len(names) == len(set(names)), "Ambiguous duplicate APK entries")
    require(sum(e.file_size for e in entries) <= 1024 ** 3, "Oversized expanded APK")
    for entry in entries:
        path = PurePosixPath(entry.filename)
        require(not path.is_absolute() and ".." not in path.parts
                and not stat.S_ISLNK(entry.external_attr >> 16), "Linked or foreign APK entry")
    libraries = [n for n in names if n.startswith("lib/") and n.endswith(".so")]
    require(libraries and all(n.startswith("lib/x86_64/") for n in libraries), "Wrong APK ABI")
    for name in libraries:
        with archive.open(name) as stream:
            header = stream.read(20)
        require(len(header) == 20 and header[:6] == b"\x7fELF\x02\x01"
                and struct.unpack("<H", header[18:20])[0] == 62, "Wrong actual ELF architecture")
    with archive.open("lib/x86_64/libtakosumi_mobile_lib.so") as stream:
        require(digest(stream) == native_hash, "APK does not contain the actual Cargo App bytes")
    require(archive.testzip() is None, "APK CRC failure")
with apk.open("rb") as stream:
    apk_hash = digest(stream)
def git(directory, *args):
    return subprocess.check_output(["git", "-C", str(directory), *args], text=True, timeout=10).strip()
subprocess.run(["git", "diff", "--exit-code", "HEAD", "--"], check=True, timeout=10)
proof = {"scope": "whole Tauri Android debug APK compile; not install/UI/device qualification",
         "head": git(root, "rev-parse", "HEAD"), "tree": git(root, "rev-parse", "HEAD^{tree}"),
         "kitHead": git(root.parent / "mobile-kit", "rev-parse", "HEAD"),
         "package": manifest.get("package"), "minSdk": 28, "targetSdk": 36,
         "signatureVerified": True, "debuggable": True, "buildType": "debug",
         "abi": "x86_64", "nativeLibraries": len(libraries),
         "extractionRulesSha256": hashlib.sha256(json.dumps(policy(rules)).encode()).hexdigest(),
         "apk": {"path": str(apk.relative_to(root)), "bytes": apk.stat().st_size, "sha256": apk_hash},
         "cargoAppSha256": native_hash}
(generated / "whole-app-proof.json").write_text(json.dumps(proof, indent=2) + "\n")
print(json.dumps(proof, indent=2))
print("PASS: whole App Android debug APK SDK build; no runtime or distribution qualification.")
PY
