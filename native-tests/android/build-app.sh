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

python3 -B native-tests/android/verify_app.py
