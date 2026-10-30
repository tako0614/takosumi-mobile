import { expect, test } from "bun:test";
import { createTauriPushNative, PushUnavailableError, type PushListen } from "../src/push-native.ts";

function fakeTauri(answers: Record<string, unknown | Error>) {
  const calls: { command: string; args?: Record<string, unknown> }[] = [];
  const handlers = new Map<string, (payload: unknown) => void>();
  const invoke = async <T>(command: string, args?: Record<string, unknown>): Promise<T> => {
    calls.push({ command, ...(args ? { args } : {}) });
    const answer = answers[command];
    if (answer instanceof Error) throw answer;
    return answer as T;
  };
  const listen: PushListen = async (plugin, event, handler) => {
    handlers.set(`${plugin}:${event}`, handler as (payload: unknown) => void);
    return { unregister: async () => void handlers.delete(`${plugin}:${event}`) };
  };
  return { calls, handlers, native: createTauriPushNative({ invoke, listen }) };
}

const P256DH = "B".padEnd(87, "A");
const AUTH = "A".repeat(22);

test("commands go to the takosumi-push plugin with their payloads", async () => {
  const tauri = fakeTauri({
    "plugin:takosumi-push|permission_state": { state: "denied" },
    "plugin:takosumi-push|request_permission": { granted: true },
    "plugin:takosumi-push|get_token": { token: "tok_abcdefghijklmnop", provider: "fcm", environment: "production" },
    "plugin:takosumi-push|device_name": { name: "  Pixel 9  " },
    "plugin:takosumi-push|create_subscription_keys": { p256dh: P256DH, auth: AUTH },
    "plugin:takosumi-push|delete_subscription_keys": undefined,
    "plugin:takosumi-push|list_subscriptions": {
      subscriptions: [
        { subscriptionId: "AbCdEfGhIjKlMnOpQrStUv", instance: "https://t.example", account: "a", label: "t" },
        { subscriptionId: "bad id", instance: "https://t.example" },
      ],
    },
    "plugin:takosumi-push|take_pending_tap": {
      tap: { id: "tap1", subscriptionId: "AbCdEfGhIjKlMnOpQrStUv", instance: "https://t.example", account: "a", navigate: "https://t.example/runs/1" },
    },
  });
  const native = tauri.native;
  expect(await native.permissionState()).toBe("denied");
  expect(await native.requestPermission()).toBe(true);
  expect(await native.getToken()).toEqual({ token: "tok_abcdefghijklmnop", provider: "fcm", environment: "production" });
  expect(await native.deviceName()).toBe("Pixel 9");
  const request = {
    subscriptionId: "AbCdEfGhIjKlMnOpQrStUv",
    instance: "https://t.example",
    account: "a",
    label: "t.example",
    origins: ["https://t.example"],
  };
  expect(await native.createSubscriptionKeys(request)).toEqual({ p256dh: P256DH, auth: AUTH });
  await native.deleteSubscriptionKeys("AbCdEfGhIjKlMnOpQrStUv");
  expect(await native.listSubscriptions()).toHaveLength(1);
  expect((await native.takePendingTap())?.navigate).toBe("https://t.example/runs/1");
  expect(tauri.calls.find((call) => call.command.endsWith("create_subscription_keys"))?.args).toEqual({ payload: request });
  expect(tauri.calls.find((call) => call.command.endsWith("delete_subscription_keys"))?.args).toEqual({
    payload: { subscriptionId: "AbCdEfGhIjKlMnOpQrStUv" },
  });
});

test("no push in this build is PushUnavailableError; malformed native answers are refused", async () => {
  const tauri = fakeTauri({
    "plugin:takosumi-push|get_token": new Error("push_unavailable: Firebase is not configured in this build"),
    "plugin:takosumi-push|create_subscription_keys": { p256dh: "short", auth: AUTH },
    "plugin:takosumi-push|take_pending_tap": { tap: { id: "t", subscriptionId: "x", instance: "https://t.example" } },
    "plugin:takosumi-push|permission_state": { state: "weird" },
  });
  await expect(tauri.native.getToken()).rejects.toBeInstanceOf(PushUnavailableError);
  await expect(
    tauri.native.createSubscriptionKeys({ subscriptionId: "AbCdEfGhIjKlMnOpQrStUv", instance: "https://t.example", account: "", label: "t", origins: ["https://t.example"] }),
  ).rejects.toThrow("invalid");
  expect(await tauri.native.takePendingTap()).toBeUndefined();
  expect(await tauri.native.permissionState()).toBe("prompt");
  const other = fakeTauri({ "plugin:takosumi-push|get_token": { token: "", provider: "fcm" } });
  await expect(other.native.getToken()).rejects.toThrow("invalid");
});

test("events are decoded; malformed ones never reach the shell", async () => {
  const tauri = fakeTauri({});
  const taps: unknown[] = [];
  const tokens: unknown[] = [];
  const stopTaps = await tauri.native.onTap((tap) => taps.push(tap));
  await tauri.native.onTokenRefresh((token) => tokens.push(token));
  const emitTap = tauri.handlers.get("takosumi-push:notification-tapped")!;
  emitTap({ id: "1", subscriptionId: "AbCdEfGhIjKlMnOpQrStUv", instance: "https://t.example", account: "" });
  emitTap({ id: "2", subscriptionId: "no", instance: "https://t.example" });
  emitTap("garbage");
  expect(taps).toHaveLength(1);
  tauri.handlers.get("takosumi-push:token-refresh")!({ token: "apns-token", provider: "apns", environment: "development" });
  tauri.handlers.get("takosumi-push:token-refresh")!({ token: "x", provider: "webpush" });
  expect(tokens).toEqual([{ token: "apns-token", provider: "apns", environment: "development" }]);
  await stopTaps();
  expect(tauri.handlers.has("takosumi-push:notification-tapped")).toBe(false);
});
