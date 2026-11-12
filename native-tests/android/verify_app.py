"""Actual whole-App APK checks shared by SDK and external UI qualification."""
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import stat
import struct
import subprocess
import xml.etree.ElementTree as ET
import zipfile

from apk_policy import extraction_resource
from fixture_network import verify_network


def require(value, message):
    if not value:
        raise RuntimeError(message)

def digest(stream):
    return hashlib.file_digest(stream, "sha256").hexdigest()

def verify_app(root, fixture_ca=None):
    root = root.resolve(strict=True)
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
    def xml(path):
        text = subprocess.check_output(
            [str(sdk / "cmdline-tools/latest/bin/apkanalyzer"), "resources", "xml", "--file",
             "/" + path, str(apk)], text=True, timeout=30)
        return ET.fromstring(text)
    rules = xml(rules_file)
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
    launchers = [n for n in app.findall("activity") if any(
        any(a.get(android + "name") == "android.intent.action.MAIN" for a in f.findall("action"))
        and any(c.get(android + "name") == "android.intent.category.LAUNCHER" for c in f.findall("category"))
        for f in n.findall("intent-filter"))]
    require(len(launchers) == 1 and launchers[0].get(android + "exported") == "true", "Unique App launcher required")
    activity = launchers[0].get(android + "name", "")
    if activity.startswith("."):
        activity = manifest.get("package") + activity
    require(activity.startswith(manifest.get("package") + ".")
            and all(c.isalnum() or c in "._$" for c in activity), "Foreign App launcher")
    target = Path(os.environ.get("CARGO_TARGET_DIR", root / "src-tauri/target"))
    require(target.resolve() == (root / "src-tauri/target").resolve(), "Foreign Cargo target refused")
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
        if fixture_ca is None:
            require(app.get(android + "networkSecurityConfig") is None, "Unexpected fixture trust in normal SDK APK")
            network = None
        else:
            network = verify_network(app, resource_table, xml, archive, fixture_ca)
    with apk.open("rb") as stream:
        apk_hash = digest(stream)
    def git(directory, *args):
        return subprocess.check_output(["git", "-C", str(directory), *args], text=True, timeout=10).strip()
    subprocess.run(["git", "diff", "--exit-code", "HEAD", "--"], cwd=root, check=True, timeout=10)
    proof = {"scope": "whole Tauri Android debug APK compile; not install/UI/device qualification",
             "head": git(root, "rev-parse", "HEAD"), "tree": git(root, "rev-parse", "HEAD^{tree}"),
             "kitHead": git(root.parent / "mobile-kit", "rev-parse", "HEAD"),
             "package": manifest.get("package"), "minSdk": 28, "targetSdk": 36,
             "signatureVerified": True, "debuggable": True, "buildType": "debug",
             "launcher": manifest.get("package") + "/" + activity, "fixtureNetworkTrust": network,
             "abi": "x86_64", "nativeLibraries": len(libraries),
             "extractionRulesSha256": hashlib.sha256(json.dumps(policy(rules)).encode()).hexdigest(),
             "apk": {"path": str(apk.relative_to(root)), "bytes": apk.stat().st_size, "sha256": apk_hash},
             "cargoAppSha256": native_hash}
    return proof


def main():
    root = Path(__file__).resolve().parents[2]
    proof = verify_app(root)
    (root / "src-tauri/gen/android/whole-app-proof.json").write_text(json.dumps(proof, indent=2) + "\n")
    print(json.dumps(proof, indent=2))
    print("PASS: whole App Android debug APK SDK build; no runtime or distribution qualification.")


if __name__ == "__main__":
    main()
