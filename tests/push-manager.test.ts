import { expect, test } from "bun:test";
import { mobileAccountIdentity, type MobileSession } from "@takosjp/mobile-kit";
import { base64UrlEncode } from "../relay/src/encoding.ts";
import {
  createPushManager,
  PUSH_STATE_STORAGE_KEY,
  PushError,
  RELAY_DEVICE_STORAGE_KEY,
  type PushManagerDeps,
} from "../src/push-manager.ts";
import { routeNotification } from "../src/push-routing.ts";
import { withAccountRemovalHook } from "../src/push-account-hook.ts";
import { productAdapter } from "../src/product.ts";
import {
  createFakeInstance,
  createFakeNative,
  createPushWorld,
  memorySecureStore,
  memoryStore,
  RELAY_ORIGIN,
  type FakeInstance,
  type FakeNative,
  type PushWorld,
} from "./support/push-world.ts";

function setup(
  options: {
    instances?: FakeInstance[];
    native?: FakeNative;
    relayOrigin?: string | null;
    secure?: boolean;
  } = {},
) {
  const instances = options.instances ?? [createFakeInstance({ origin: "https://takosumi.example" })];
  const world = createPushWorld(instances);
  const native = options.native ?? createFakeNative();
  const store = memoryStore();
  const secureStore = memorySecureStore();
  const relayDown = { value: false };
  const deps: PushManagerDeps = {
    native: native.native,
    relayOrigin: options.relayOrigin === null ? undefined : (options.relayOrigin ?? RELAY_ORIGIN),
    fetch: async (input, init) => {
      if (relayDown.value && input.startsWith(RELAY_ORIGIN)) throw new TypeError("network down");
      return await world.fetch(input, init);
    },
    store,
    secureStore: options.secure === false ? undefined : secureStore,
    freshSession: async (session) => session,
    lang: () => "ja-JP",
    now: () => new Date(world.relay.clock.now * 1000),
  };
  const manager = createPushManager(deps);
  return { instances, instance: instances[0]!, world, native, store, secureStore, manager, relayDown };
}

/** A bridge whose saved-account list files `rows` (slot id → account). */
async function hookBridge(rows: readonly { id: string; hostUrl: string; accountId?: string }[]) {
  const secure = memorySecureStore();
  const storage = memoryStore();
  await storage.set(
    "takosumi.mobile.takosumi.accounts",
    JSON.stringify({ version: 1, accounts: rows.map((row) => ({ ...row, product: "takosumi", lastUsedAt: "2027-01-01T00:00:00.000Z" })) }),
  );
  return {
    secure,
    bridge: {
      capabilities: {} as never,
      secureStore: secure,
      storage,
      getLaunchPayload: async () => undefined,
      openExternalUrl: async () => undefined,
    },
  };
}

function relaySubscriptions(world: PushWorld): number {
  return (world.relay.sqlite.query("SELECT COUNT(*) AS n FROM subscriptions").get() as { n: number }).n;
}

function relayDevices(world: PushWorld): number {
  return (world.relay.sqlite.query("SELECT COUNT(*) AS n FROM devices").get() as { n: number }).n;
}

test("turning on registers the device once, subscribes with the instance key and registers the pusher", async () => {
  const { instance, world, native, store, secureStore, manager } = setup();
  const session = instance.session();
  const record = await manager.enable(session);

  // One relay device; its secret only in the secure store.
  expect(relayDevices(world)).toBe(1);
  const device = JSON.parse(secureStore.values.get(RELAY_DEVICE_STORAGE_KEY)!);
  expect(device.relayOrigin).toBe(RELAY_ORIGIN);
  expect(store.values.get(PUSH_STATE_STORAGE_KEY)).not.toContain(device.deviceSecret);
  expect([...store.values.values()].join("")).not.toContain(device.deviceSecret);

  // The relay subscription carries the instance's VAPID key.
  const subscription = world.relay.sqlite
    .query("SELECT application_server_key AS key FROM subscriptions WHERE subscription_id = ?")
    .get(record.subscriptionId) as { key: string };
  const config = await (await world.fetch("https://takosumi.example/api/v1/account/notification-pushers", {
    headers: { authorization: "Bearer access-token-1" },
  })).json() as { web_push_public_key: string };
  expect(subscription.key).toBe(config.web_push_public_key);
  expect(record.instanceKey).toBe(config.web_push_public_key);

  // Native keys belong to this instance and account, with its origins.
  const key = native.keys.get(record.subscriptionId)!;
  expect(key.request).toEqual({
    subscriptionId: record.subscriptionId,
    instance: "https://takosumi.example",
    account: "acct_1",
    label: "takosumi.example (acct_1@example)",
    origins: ["https://takosumi.example"],
  });

  // The pusher: MSC4174 fields, Takosumi Mobile's app id.
  expect(instance.pushers).toHaveLength(1);
  const pusher = instance.pushers[0]!;
  expect(pusher.app_id).toBe("com.takosumi");
  expect(pusher.pushkey).toBe(base64UrlEncode(key.publicKey));
  expect(pusher.data).toEqual({ url: record.endpoint, auth: base64UrlEncode(key.auth) });
  expect(pusher.device_display_name).toBe("Pixel Test");
  expect(pusher.lang).toBe("ja-JP");
  expect(record.endpoint.startsWith(`${RELAY_ORIGIN}/push/`)).toBe(true);
  expect(record.pusherId).toBe(pusher.id);
  expect(await manager.isEnabled(session)).toBe(true);
});

test("an approval wait travels instance → relay → device, decrypts, and its tap opens the Run", async () => {
  const { instance, world, native, manager } = setup();
  const session = instance.session();
  await manager.enable(session);

  const response = await instance.send(world, instance.pushers[0]!, {
    title: "承認を待っています",
    body: "yurucommu の変更に、削除を含むplanがあります",
    navigate: "https://takosumi.example/runs/run_123",
    lang: "ja-JP",
    tag: "run:run_123",
  });
  expect(response.status).toBe(201);
  const delivery = world.lastDelivery()!;
  // The relay carried only {s, b}: ciphertext, never the text.
  expect(JSON.stringify(delivery)).not.toContain("承認");

  const shown = await native.receive(delivery);
  expect(shown).toMatchObject({
    generic: false,
    title: "承認を待っています",
    navigate: "https://takosumi.example/runs/run_123",
    instance: "https://takosumi.example",
    account: "acct_1",
  });
  const tap = native.tap(shown);
  expect(routeNotification(tap.navigate, ["https://takosumi.example"])).toEqual({
    kind: "run",
    runId: "run_123",
    url: "https://takosumi.example/runs/run_123",
  });
});

test("each instance and account gets its own subscription on the one relay device", async () => {
  const a = createFakeInstance({ origin: "https://a.example" });
  const b = createFakeInstance({ origin: "https://b.example" });
  const { world, native, manager } = setup({ instances: [a, b] });
  const first = await manager.enable(a.session("alice"));
  const second = await manager.enable(b.session("bob"));
  const third = await manager.enable(a.session("carol"));
  expect(relayDevices(world)).toBe(1);
  expect(relaySubscriptions(world)).toBe(3);
  expect(new Set([first.subscriptionId, second.subscriptionId, third.subscriptionId]).size).toBe(3);
  expect(native.keys.size).toBe(3);
  expect(a.pushers).toHaveLength(2);
  expect(b.pushers).toHaveLength(1);

  // A message for one does not decrypt as another.
  await b.send(world, b.pushers[0]!, { title: "B", navigate: "https://b.example/apps/x" });
  const delivery = world.lastDelivery()!;
  expect(delivery.s).toBe(second.subscriptionId);
  const forged = await native.receive({ s: first.subscriptionId, b: delivery.b });
  expect(forged.generic).toBe(true);
});

test("turning off deletes the pusher, the relay subscription and the keys", async () => {
  const { instance, world, native, manager } = setup();
  const session = instance.session();
  const record = await manager.enable(session);
  const pusher = { ...instance.pushers[0]! };
  await manager.disable(session);
  expect(instance.pushers).toHaveLength(0);
  expect(relaySubscriptions(world)).toBe(0);
  expect(native.keys.size).toBe(0);
  expect(await manager.isEnabled(session)).toBe(false);
  // The old endpoint is gone: an instance still holding it gets 404.
  instance.pushers.push(pusher);
  expect((await instance.send(world, pusher, { title: "x", navigate: "https://takosumi.example/" })).status).toBe(404);
  expect(record.subscriptionId).toBeTruthy();
});

test("a refused notification permission stops before anything is registered", async () => {
  const native = createFakeNative({ permission: "prompt", grant: false });
  const { instance, world, manager } = setup({ native });
  const failure = await manager.enable(instance.session()).catch((error) => error);
  expect(failure).toBeInstanceOf(PushError);
  expect(failure.problem).toBe("permission_denied");
  expect(relayDevices(world)).toBe(0);
  expect(instance.pushers).toHaveLength(0);
});

test("an instance that cannot send says why, before the relay is touched", async () => {
  for (const [reason, problem] of [
    ["relay_not_configured", "instance_relay_not_configured"],
    ["secret_not_configured", "instance_secret_not_configured"],
    ["issuer_not_https", "instance_issuer_not_https"],
  ] as const) {
    const instance = createFakeInstance({ origin: "https://takosumi.example", unavailable: reason });
    const { world, native, manager } = setup({ instances: [instance] });
    const failure = await manager.enable(instance.session()).catch((error) => error);
    expect(failure.problem).toBe(problem);
    expect(failure.message).toBeTruthy();
    expect(relayDevices(world)).toBe(0);
    expect(native.keys.size).toBe(0);
  }
});

test("an instance that does not allow this relay leaves nothing behind", async () => {
  const instance = createFakeInstance({ origin: "https://takosumi.example", relayOrigins: ["https://push.other.example"] });
  const { world, native, manager } = setup({ instances: [instance] });
  const failure = await manager.enable(instance.session()).catch((error) => error);
  expect(failure.problem).toBe("relay_not_allowed");
  expect(relaySubscriptions(world)).toBe(0);
  expect(native.keys.size).toBe(0);
  expect(instance.pushers).toHaveLength(0);
});

test("no FCM/APNs on this device is reported, not retried", async () => {
  const native = createFakeNative({ token: "unavailable" });
  const { instance, world, manager } = setup({ native });
  const failure = await manager.enable(instance.session()).catch((error) => error);
  expect(failure.problem).toBe("push_unavailable");
  expect(relayDevices(world)).toBe(0);
});

test("a rotated instance key is noticed on the next check: new subscription and pusher, old ones deleted", async () => {
  const { instance, world, native, manager } = setup();
  const session = instance.session();
  const before = await manager.enable(session);
  expect(await manager.check(session)).toEqual({ kind: "ok" });

  await instance.rotateKey();
  expect(await manager.check(session)).toEqual({ kind: "resubscribed" });
  const after = (await manager.record(session))!;
  expect(after.subscriptionId).not.toBe(before.subscriptionId);
  expect(after.instanceKey).not.toBe(before.instanceKey);
  expect(instance.pushers.map((pusher) => pusher.id)).toEqual([after.pusherId]);
  expect(relaySubscriptions(world)).toBe(1);
  expect([...native.keys.keys()]).toEqual([after.subscriptionId]);
  // And the new key works end to end.
  expect((await instance.send(world, instance.pushers[0]!, { title: "ok", navigate: "https://takosumi.example/" })).status).toBe(201);
  expect((await native.receive(world.lastDelivery()!)).generic).toBe(false);
});

test("a pusher removed on the instance (account settings) turns this device off", async () => {
  const { instance, world, native, manager } = setup();
  const session = instance.session();
  await manager.enable(session);
  instance.pushers.splice(0, 1);
  expect(await manager.check(session)).toEqual({ kind: "removed" });
  expect(await manager.isEnabled(session)).toBe(false);
  expect(relaySubscriptions(world)).toBe(0);
  expect(native.keys.size).toBe(0);
});

test("a removed pusher is not quietly registered again, even when the key also changed", async () => {
  const { instance, world, native, manager } = setup();
  const session = instance.session();
  await manager.enable(session);
  instance.pushers.splice(0, 1);
  await instance.rotateKey();
  native.keys.clear();
  expect(await manager.check(session)).toEqual({ kind: "removed" });
  expect(instance.pushers).toHaveLength(0);
  expect(await manager.isEnabled(session)).toBe(false);
  expect(relaySubscriptions(world)).toBe(0);
});

test("a stale subscription whose pusher the instance dropped (relay 404) subscribes again", async () => {
  const a = createFakeInstance({ origin: "https://a.example" });
  const b = createFakeInstance({ origin: "https://b.example" });
  const { world, manager } = setup({ instances: [a, b] });
  await manager.enable(a.session());
  const old = { ...a.pushers[0]! };
  world.relay.sqlite.query("DELETE FROM subscriptions").run();
  world.relay.sqlite.query("DELETE FROM devices").run();
  // The instance sends, gets 404, and drops the pusher.
  expect((await a.send(world, old, { title: "x", navigate: "https://a.example/" })).status).toBe(404);
  expect(a.pushers).toHaveLength(0);
  await manager.enable(b.session());
  expect((await manager.record(a.session()))!.stale).toBe(true);
  expect(await manager.check(a.session())).toEqual({ kind: "resubscribed" });
  expect(a.pushers).toHaveLength(1);
});

test("a pusher the instance dropped because the relay forgot the device is renewed, not reported as removed", async () => {
  const { instance, world, native, manager } = setup();
  const session = instance.session();
  const before = await manager.enable(session);
  const old = { ...instance.pushers[0]! };
  // The relay purged the device; the instance's next send gets 404 and drops the pusher.
  world.relay.sqlite.query("DELETE FROM subscriptions").run();
  world.relay.sqlite.query("DELETE FROM devices").run();
  expect((await instance.send(world, old, { title: "x", navigate: "https://takosumi.example/" })).status).toBe(404);
  expect(instance.pushers).toHaveLength(0);
  // The PUT probe answers 401: register again and subscribe again.
  expect(await manager.check(session)).toEqual({ kind: "resubscribed" });
  const after = (await manager.record(session))!;
  expect(after.subscriptionId).not.toBe(before.subscriptionId);
  expect(instance.pushers.map((pusher) => pusher.id)).toEqual([after.pusherId]);
  expect([...native.keys.keys()]).toEqual([after.subscriptionId]);
  expect((await instance.send(world, instance.pushers[0]!, { title: "ok", navigate: "https://takosumi.example/" })).status).toBe(201);
});

test("a missing pusher with the relay out of reach changes nothing", async () => {
  const { instance, native, manager, relayDown } = setup();
  const session = instance.session();
  await manager.enable(session);
  instance.pushers.splice(0, 1);
  relayDown.value = true;
  expect(await manager.check(session)).toEqual({ kind: "unknown" });
  expect(await manager.isEnabled(session)).toBe(true);
  expect(native.keys.size).toBe(1);
  relayDown.value = false;
  expect(await manager.check(session)).toEqual({ kind: "removed" });
});

test("reads wait for a change in progress", async () => {
  const { instance, manager } = setup();
  const session = instance.session();
  const enabling = manager.enable(session);
  // Queued behind the enable: never the half-written state.
  expect(await manager.isEnabled(session)).toBe(true);
  expect(await manager.shouldPrompt(session)).toBe(false);
  await enabling;
  const disabling = manager.disable(session);
  expect(await manager.isEnabled(session)).toBe(false);
  await disabling;
});

test("lost native keys are replaced on the next check", async () => {
  const { instance, native, manager } = setup();
  const session = instance.session();
  const before = await manager.enable(session);
  native.keys.clear();
  expect(await manager.check(session)).toEqual({ kind: "resubscribed" });
  expect((await manager.record(session))!.subscriptionId).not.toBe(before.subscriptionId);
  expect(native.keys.size).toBe(1);
});

test("a new FCM token goes to the relay with PUT /v1/devices/{id}", async () => {
  const { instance, world, native, secureStore, manager } = setup();
  await manager.enable(instance.session());
  const device = JSON.parse(secureStore.values.get(RELAY_DEVICE_STORAGE_KEY)!);
  const next = { token: "fcm-token_device-one:rotated-0123456789", provider: "fcm" as const, environment: "production" as const };
  native.setToken(next);
  await manager.tokenRefreshed(next);
  const row = world.relay.sqlite.query("SELECT token FROM devices WHERE device_id = ?").get(device.deviceId) as { token: string };
  expect(row.token).toBe(next.token);
  expect(JSON.parse(secureStore.values.get(RELAY_DEVICE_STORAGE_KEY)!).token).toBe(next.token);
  // Still delivered.
  expect((await instance.send(world, instance.pushers[0]!, { title: "t", navigate: "https://takosumi.example/" })).status).toBe(201);
  expect(world.relay.fcm.requests.at(-1)!.token).toBe(next.token);
});

test("a relay that forgot the device: register again, and other instances resubscribe on their check", async () => {
  const a = createFakeInstance({ origin: "https://a.example" });
  const b = createFakeInstance({ origin: "https://b.example" });
  const { world, secureStore, manager } = setup({ instances: [a, b] });
  const first = await manager.enable(a.session());
  const device = JSON.parse(secureStore.values.get(RELAY_DEVICE_STORAGE_KEY)!);
  // The daily purge (or a DELETE) removed the device and its subscriptions.
  world.relay.sqlite.query("DELETE FROM subscriptions").run();
  world.relay.sqlite.query("DELETE FROM devices").run();

  await manager.enable(b.session());
  const again = JSON.parse(secureStore.values.get(RELAY_DEVICE_STORAGE_KEY)!);
  expect(again.deviceId).not.toBe(device.deviceId);
  expect((await manager.record(a.session()))!.stale).toBe(true);

  expect(await manager.check(a.session())).toEqual({ kind: "resubscribed" });
  const now = (await manager.record(a.session()))!;
  expect(now.subscriptionId).not.toBe(first.subscriptionId);
  expect(now.stale).toBeUndefined();
  expect(a.pushers.map((pusher) => pusher.id)).toEqual([now.pusherId]);
  expect((await a.send(world, a.pushers[0]!, { title: "x", navigate: "https://a.example/" })).status).toBe(201);
});

test("signing out removes the instance's pusher, relay subscription and keys before the session is deleted", async () => {
  const { instance, world, native, manager } = setup();
  const session = instance.session();
  await manager.enable(session);

  const slotKey = "takosumi.mobile.takosumi.session.AbCdEfGhIjKlMnOp";
  const { secure, bridge: base } = await hookBridge([
    { id: "AbCdEfGhIjKlMnOp", hostUrl: "https://takosumi.example", accountId: "acct_1" },
  ]);
  await secure.set(slotKey, JSON.stringify(session));
  await secure.set("unrelated", "x");
  const seen: MobileSession[] = [];
  const bridge = withAccountRemovalHook(
    base,
    productAdapter,
    async (removed) => {
      seen.push(removed);
      // The session is still readable while the cleanup runs.
      expect(await secure.get(slotKey)).toBeDefined();
      await manager.accountRemoving(removed);
    },
  );
  await bridge.secureStore!.delete("unrelated");
  expect(seen).toHaveLength(0);
  await bridge.secureStore!.delete(slotKey);
  expect(seen.map((item) => mobileAccountIdentity(item))).toEqual([mobileAccountIdentity(session)]);
  expect(await secure.get(slotKey)).toBeUndefined();
  expect(instance.pushers).toHaveLength(0);
  expect(relaySubscriptions(world)).toBe(0);
  expect(native.keys.size).toBe(0);
});

test("a slot dropped because its stored session is another account's keeps that account's notifications", async () => {
  const a = createFakeInstance({ origin: "https://a.example" });
  const { world, native, manager } = setup({ instances: [a] });
  const alice = a.session("alice");
  await manager.enable(alice);
  // Bob's row, but the slot holds Alice's session (mobile-kit discards it).
  const slotKey = "takosumi.mobile.takosumi.session.BobSlot0123456789";
  const { secure, bridge: base } = await hookBridge([
    { id: "BobSlot0123456789", hostUrl: "https://a.example", accountId: "bob" },
    { id: "AliceSlot01234567", hostUrl: "https://a.example", accountId: "alice" },
  ]);
  await secure.set(slotKey, JSON.stringify(alice));
  let called = 0;
  const bridge = withAccountRemovalHook(base, productAdapter, async (removed) => {
    called += 1;
    await manager.accountRemoving(removed);
  });
  await bridge.secureStore!.delete(slotKey);
  expect(called).toBe(0);
  expect(await secure.get(slotKey)).toBeUndefined();
  expect(await manager.isEnabled(alice)).toBe(true);
  expect(a.pushers).toHaveLength(1);
  expect(relaySubscriptions(world)).toBe(1);
  expect(native.keys.size).toBe(1);
  // A slot with no row at all is not an account either.
  await secure.set("takosumi.mobile.takosumi.session.NoRow0123456789A", JSON.stringify(alice));
  await bridge.secureStore!.delete("takosumi.mobile.takosumi.session.NoRow0123456789A");
  expect(called).toBe(0);
});

test("a sign-out never waits long for the cleanup", async () => {
  const slotKey = "takosumi.mobile.takosumi.session.AbCdEfGhIjKlMnOp";
  const { secure, bridge: base } = await hookBridge([
    { id: "AbCdEfGhIjKlMnOp", hostUrl: "https://t.example", accountId: "acct_1" },
  ]);
  await secure.set(slotKey, JSON.stringify(createFakeInstance({ origin: "https://t.example" }).session()));
  const bridge = withAccountRemovalHook(
    base,
    productAdapter,
    () => new Promise<void>(() => undefined),
    20,
  );
  await bridge.secureStore!.delete(slotKey);
  expect(await secure.get(slotKey)).toBeUndefined();
});

test("launch reconciliation drops accounts that are gone, orphaned keys and earlier failed deletions", async () => {
  const a = createFakeInstance({ origin: "https://a.example" });
  const b = createFakeInstance({ origin: "https://b.example" });
  const { world, native, manager } = setup({ instances: [a, b] });
  await manager.enable(a.session());
  const kept = await manager.enable(b.session());
  // An orphan: keys without a record (the app died mid-way).
  await native.native.createSubscriptionKeys({
    subscriptionId: "orphan_subscription_1",
    instance: "https://c.example",
    account: "",
    label: "c.example",
    origins: ["https://c.example"],
  });
  // Only b is still a saved account.
  await manager.reconcile([b.session()]);
  expect(await manager.isEnabled(a.session())).toBe(false);
  expect([...native.keys.keys()]).toEqual([kept.subscriptionId]);
  expect(relaySubscriptions(world)).toBe(1);
});

test("asks once per instance and account after sign-in", async () => {
  const { instance, manager } = setup();
  const alice = instance.session("alice");
  const bob = instance.session("bob");
  expect(await manager.shouldPrompt(alice)).toBe(true);
  await manager.markPrompted(alice);
  expect(await manager.shouldPrompt(alice)).toBe(false);
  expect(await manager.shouldPrompt(bob)).toBe(true);
  await manager.enable(bob);
  expect(await manager.shouldPrompt(bob)).toBe(false);
});

test("without the native plugin, a relay or a secure store, notifications say why", async () => {
  expect(createPushManager({ ...baseDeps(), native: undefined }).availability()).toBe("unsupported");
  expect(createPushManager({ ...baseDeps(), relayOrigin: undefined }).availability()).toBe("relay_not_built");
  expect(createPushManager({ ...baseDeps(), secureStore: undefined }).availability()).toBe("no_secure_storage");
  const manager = createPushManager({ ...baseDeps(), relayOrigin: undefined });
  const session = createFakeInstance({ origin: "https://t.example" }).session();
  expect((await manager.enable(session).catch((error) => error)).problem).toBe("relay_not_built");
  expect(await manager.shouldPrompt(session)).toBe(false);
  expect(await manager.check(session)).toEqual({ kind: "off" });
});

function baseDeps(): PushManagerDeps {
  return {
    native: createFakeNative().native,
    relayOrigin: RELAY_ORIGIN,
    fetch: async () => new Response(null, { status: 500 }),
    store: memoryStore(),
    secureStore: memorySecureStore(),
    freshSession: async (session) => session,
    lang: () => "en",
  };
}
