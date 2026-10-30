import { expect, test } from "bun:test";

/**
 * The takosumi-push plugin's configuration: its commands exist in Rust,
 * permissions and both native sides; only the shell's mobile capability
 * grants them; no command hands a private key to JavaScript; the messaging
 * service is not exported; notification taps carry only an opaque token.
 */

const root = new URL("../src-tauri/", import.meta.url);
const read = (path: string) => Bun.file(new URL(path, root)).text();

const COMMANDS = [
  "permission_state",
  "request_permission",
  "get_token",
  "device_name",
  "create_subscription_keys",
  "delete_subscription_keys",
  "list_subscriptions",
  "take_pending_tap",
];

const camel = (name: string) => name.replace(/_([a-z])/gu, (_, letter: string) => letter.toUpperCase());

test("every command is declared, permitted, handled and implemented natively", async () => {
  const build = await read("plugins/push/build.rs");
  const permissions = await read("plugins/push/permissions/default.toml");
  const lib = await read("plugins/push/src/lib.rs");
  const mobile = await read("plugins/push/src/mobile.rs");
  const kotlin = await read("plugins/push/android/src/main/java/PushPlugin.kt");
  const swift = await read("plugins/push/ios/Sources/TakosumiPush/PushPlugin.swift");
  for (const command of COMMANDS) {
    expect(build).toContain(`"${command}"`);
    expect(permissions).toContain(`"allow-${command.replaceAll("_", "-")}"`);
    expect(lib).toContain(`commands::${command}`);
    expect(mobile).toContain(`"${camel(command)}"`);
    expect(kotlin).toContain(`fun ${camel(command)}(invoke: Invoke)`);
    expect(swift).toContain(`func ${camel(command)}(_ invoke: Invoke)`);
  }
  expect(build).toContain('"register_listener"');
  expect(permissions).toContain('"allow-register-listener"');
});

test("only the shell's mobile capability grants the plugin", async () => {
  const mobile = JSON.parse(await read("capabilities/mobile.json")) as { permissions: unknown[]; windows: string[] };
  const desktop = JSON.parse(await read("capabilities/default.json")) as { permissions: unknown[] };
  expect(mobile.permissions).toContain("takosumi-push:default");
  expect(mobile.windows).toEqual(["main"]);
  expect(desktop.permissions.some((item) => typeof item === "string" && item.startsWith("takosumi-push:"))).toBe(false);
  const cargo = await read("Cargo.toml");
  const mobileDeps = cargo.slice(cargo.indexOf("[target.'cfg(any(target_os = \"android\", target_os = \"ios\"))'.dependencies]"));
  expect(mobileDeps).toContain('tauri-plugin-takosumi-push = { path = "plugins/push" }');
  expect(await read("src/lib.rs")).toContain("tauri_plugin_takosumi_push::init()");
});

test("private keys never reach JavaScript and taps carry only a token", async () => {
  const kotlin = await read("plugins/push/android/src/main/java/PushPlugin.kt");
  const swift = await read("plugins/push/ios/Sources/TakosumiPush/PushPlugin.swift");
  const models = await read("plugins/push/src/models.rs");
  // The keys answer is the public half only.
  expect(models).toMatch(/pub struct KeysResponse \{\s*pub p256dh: String,\s*pub auth: String,\s*\}/u);
  for (const source of [kotlin, swift]) {
    expect(source).not.toMatch(/put\("private|"privateKey":|wrappedPrivateKey"\s*,/u);
  }
  // Nothing is logged natively (no message text, no keys, no tokens).
  for (const file of [
    "PushPlugin.kt",
    "PushReceiver.kt",
    "PushStore.kt",
    "PushNotifier.kt",
    "TakosumiMessagingService.kt",
    "WebPushCrypto.kt",
    "DeclarativePush.kt",
  ]) {
    const source = await read(`plugins/push/android/src/main/java/${file}`);
    expect({ file, logs: /\bLog\.[dviwe]\(|println\(|printStackTrace/u.test(source) }).toEqual({ file, logs: false });
  }
  const notifier = await read("plugins/push/android/src/main/java/PushNotifier.kt");
  expect(notifier).toContain("putExtra(EXTRA_TAP_TOKEN, ticket.token)");
  expect(notifier.match(/putExtra\(/gu)).toHaveLength(1);
  expect(notifier).toContain("PendingIntent.FLAG_IMMUTABLE");
});

test("only a message that decrypts may switch accounts or open a page", async () => {
  // iOS: the APNs envelope (`s`, `b`, anything else) is set by the relay.
  const swift = await read("plugins/push/ios/Sources/TakosumiPush/PushPlugin.swift");
  const tapped = swift.slice(swift.indexOf("fileprivate func tapped"), swift.indexOf("fileprivate func received"));
  expect(tapped).toContain("authenticated(userInfo)");
  expect(swift).toContain("TakosumiWebPush.decrypt(body, subscription: subscription)");
  expect(swift).not.toContain("takosumi_navigate");
  // SecTask's entitlement API is macOS-only; iOS reads the build or profile.
  expect(swift).not.toMatch(/SecTask[A-Za-z]*\(/u);
  expect(swift).toContain("embedded");
  // Its notifications go with a subscription.
  expect(swift).toContain("removeDeliveredNotifications");
  const shared = await read("plugins/push/ios/Sources/TakosumiPushKeychain/TakosumiWebPush.swift");
  expect(shared).toContain("Content-Encoding: aes128gcm");
  // Android: an unknown `s` shows nothing; an unreadable message opens only the app.
  const receiver = await read("plugins/push/android/src/main/java/PushReceiver.kt");
  expect(receiver).toMatch(/ReceivedMessage\.Unknown -> Unit/u);
  expect(receiver).toMatch(/is ReceivedMessage\.Unreadable -> notifier\.showGeneric\(message\.record\)/u);
  expect(receiver.match(/rememberTap\(/gu)).toHaveLength(1);
  const notifier = await read("plugins/push/android/src/main/java/PushNotifier.kt");
  // One PendingIntent per notification: no request codes from hashes.
  expect(notifier).not.toContain("hashCode()");
  expect(notifier).toContain("ticket?.requestCode");
});

test("CI runs the native tests that bun run check does not", async () => {
  const workflow = await Bun.file(new URL("../.github/workflows/ci.yml", import.meta.url)).text();
  expect(workflow).toContain("src-tauri/plugins/push/jvm-tests");
  expect(workflow).toMatch(/cargo test --manifest-path "src-tauri\/plugins\/\$plugin\/Cargo\.toml"/u);
  const harness = await read("plugins/push/jvm-tests/build.gradle");
  for (const file of ["WebPushCrypto.kt", "DeclarativePush.kt", "WebPushCryptoTest.kt", "WebPushCasesTest.kt"]) {
    expect(harness).toContain(`"${file}"`);
  }
  // The pure files stay free of Android imports so the harness can build them.
  for (const file of ["WebPushCrypto.kt", "DeclarativePush.kt"]) {
    expect(await read(`plugins/push/android/src/main/java/${file}`)).not.toMatch(/^import android/mu);
  }
});

test("the messaging service is not exported and FCM stays off until notifications are turned on", async () => {
  const manifest = await read("plugins/push/android/src/main/AndroidManifest.xml");
  expect(manifest).toMatch(/android:name="com\.takosumi\.push\.TakosumiMessagingService"\s+android:exported="false"/u);
  expect(manifest).toMatch(/firebase_messaging_auto_init_enabled"\s+android:value="false"/u);
  expect(manifest).toContain("android.permission.POST_NOTIFICATIONS");
  const store = await read("plugins/push/android/src/main/java/PushStore.kt");
  // Keys live outside backups and are wrapped by the Android Keystore.
  expect(store).toContain("noBackupFilesDir");
  expect(store).toContain('"AndroidKeyStore"');
  const keychain = await read("plugins/push/ios/Sources/TakosumiPushKeychain/TakosumiPushKeychain.swift");
  expect(keychain).toContain("kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly");
  expect(keychain).toContain("kSecAttrAccessGroup");
});
