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
binary = bundle / info["CFBundleExecutable"]
assert binary.is_file() and binary.stat().st_size > 0, binary
assert not list(Path("src-tauri/gen/apple/build").rglob("*.ipa")), "Unexpected device IPA"
print(f"PASS: whole App iOS Simulator SDK build: {bundle}")
print(f"Executable bytes={binary.stat().st_size}, sha256={hashlib.sha256(binary.read_bytes()).hexdigest()}")
print("No Simulator runtime, real IdP, push, signing or distribution qualification.")
PY
