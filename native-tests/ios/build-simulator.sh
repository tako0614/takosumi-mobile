#!/usr/bin/env bash
# Whole App SDK compile only. Never signs, installs, launches or publishes.
set -euo pipefail

# The pinned CLI reads/imports IOS_* signing inputs before its no_sign branch.
# Refuse them rather than relying on --no-sign in a release-configured shell.
for signing_input in IOS_CERTIFICATE IOS_CERTIFICATE_PASSWORD IOS_MOBILE_PROVISION \
  APPLE_DEVELOPMENT_TEAM APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH; do
  if printenv "$signing_input" > /dev/null; then
    echo "Refusing signing input: $signing_input" >&2
    exit 2
  fi
done
if [[ "$(uname -s)" != Darwin ]]; then
  echo 'This check requires macOS with the iOS SDK.' >&2
  exit 2
fi
cd "$(dirname "$0")/../.."
if [[ -e src-tauri/gen/apple || -L src-tauri/gen/apple ]]; then
  echo 'Use a disposable checkout without an existing generated Apple project.' >&2
  exit 2
fi

case "$(uname -m)" in
  arm64) simulator_target=aarch64-sim; rust_target=aarch64-apple-ios-sim ;;
  x86_64) simulator_target=x86_64; rust_target=x86_64-apple-ios ;;
  *) echo 'Unsupported macOS host architecture.' >&2; exit 2 ;;
esac
xcodebuild -version
xcrun --sdk iphonesimulator --show-sdk-path
pod --version
rustup target add "$rust_target"

# A private build identity, consistent across project generation and compile.
# No Apple account, certificate, provisioning profile or push config is used.
simulator_config='{"identifier":"com.takosumi.fixture.simulator"}'
bun ../mobile-kit/scripts/init-tauri-mobile-native.mjs --platform ios \
  --apple-environment development -- --ci --skip-targets-install --config "$simulator_config"

# Xcode 26 can resolve the generic archive destination to iphoneos even when
# cargo-mobile2 requests the Simulator SDK/architecture. Restrict this fresh
# fixture project's supported platform before the normal Tauri build; retain
# its Rust build hook, plugins, push wiring and no-sign handling.
python3 - <<'PY'
from pathlib import Path
import re

spec = Path("src-tauri/gen/apple/project.yml")
text = spec.read_text()
identifiers = re.findall(r"(?m)^[ \t]*PRODUCT_BUNDLE_IDENTIFIER[ \t]*:[ \t]*(.+)$", text)
assert identifiers == ["com.takosumi.fixture.simulator"], "Not the fixture project"
marker = "    settings:\n      base:\n"
assert text.count(marker) == 1, "Unexpected generated target settings"
assert not re.search(r"(?m)^[ \t]*['\"]?(?:SDKROOT|SUPPORTED_PLATFORMS)(?:\[[^\r\n]*\])?['\"]?[ \t]*:", text), "Unexpected platform overrides"
text = text.replace(marker, marker + "        SDKROOT: iphonesimulator\n        SUPPORTED_PLATFORMS: iphonesimulator\n", 1)
spec.write_text(text)
PY
xcodegen generate --spec src-tauri/gen/apple/project.yml --project src-tauri/gen/apple
# XcodeGen rewrites entitlements from its spec. Reapply the same product writer
# used by init so regeneration cannot silently remove normal plugin wiring.
bun ../mobile-kit/scripts/apply-tauri-mobile-push-native.mjs --platform ios \
  --apple-environment development --strict
python3 - <<'PY'
from pathlib import Path
import plistlib

entitlements = list(Path("src-tauri/gen/apple").rglob("*.entitlements"))
assert len(entitlements) == 1, "Expected the generated App entitlements"
with entitlements[0].open("rb") as source:
    assert plistlib.load(source).get("aps-environment") == "development", "Missing normal push wiring"
PY
bunx tauri ios build --ci --debug --no-sign --target "$simulator_target" \
  --config "$simulator_config" -- --locked

python3 - <<'PY'
import hashlib
import plistlib
from pathlib import Path

bundles = list(Path("src-tauri/gen/apple/build").rglob("*.app"))
assert len(bundles) == 1, f"Expected exactly one Simulator App, got {bundles}"
bundle = bundles[0]
with (bundle / "Info.plist").open("rb") as source:
    info = plistlib.load(source)
assert info["CFBundleIdentifier"] == "com.takosumi.fixture.simulator", info
assert info["CFBundleSupportedPlatforms"] == ["iPhoneSimulator"], info
with Path("src-tauri/Info.ios.plist").open("rb") as source:
    privacy = plistlib.load(source)
for purpose in ("NSCameraUsageDescription", "NSMicrophoneUsageDescription", "NSFaceIDUsageDescription"):
    expected = privacy.get(purpose)
    assert isinstance(expected, str) and expected.strip(), f"Missing product purpose: {purpose}"
    assert info.get(purpose) == expected, f"App bundle lost product purpose: {purpose}"
binary = bundle / info["CFBundleExecutable"]
assert binary.is_file() and binary.stat().st_size > 0, binary
assert not list(Path("src-tauri/gen/apple/build").rglob("*.ipa")), "Unexpected device IPA"
print(f"PASS: whole App iOS Simulator SDK build: {bundle}")
print(f"Executable bytes={binary.stat().st_size}, sha256={hashlib.sha256(binary.read_bytes()).hexdigest()}")
print("No Simulator runtime, real IdP, push, signing or distribution qualification.")
PY
