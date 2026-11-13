#!/usr/bin/env -S node --experimental-strip-types
// Isolated Chrome + local Vite; no dependency install, shared browser or login.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, mkdir, open, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn, type ChildProcess } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";
import { createServer as reservePort } from "node:net";
import { fileURLToPath } from "node:url";
import { createServer } from "vite";
import { confirmBrowserCleanup } from "./lifecycle.ts";
import { createStartupObservation, guardRunning, initialPage } from "./startup.ts";

const root = fileURLToPath(new URL("../../", import.meta.url));
const output = process.argv[2];
assert(output?.startsWith("/"), "Pass a new absolute artifact directory");
await mkdir(output, { mode: 0o700 });
const temporary = await mkdtemp(join(process.env.RUNNER_TEMP || process.env.TMPDIR || tmpdir(), "sumi-webhome-ui-"));
const sourceFiles = ["src/web-home.tsx", "src/web-home.css", "src/web-connections.ts", "src/web-profile-data.ts",
  "native-tests/browser/web-home.html", "native-tests/browser/web-home.fixture.tsx", "native-tests/browser/run.ts",
  "native-tests/browser/tsconfig.json", "native-tests/browser/guard.py", "native-tests/browser/lifecycle.ts",
  "native-tests/browser/startup.ts", "native-tests/desktop/web_home.py", "vite.config.ts", "bun.lock"];
async function sourceHashes() {
  return Object.fromEntries(await Promise.all(sourceFiles.map(async (path) => [path,
    createHash("sha256").update(await readFile(`${root}${path}`)).digest("hex")])));
}
const before = await sourceHashes();
const reservation = reservePort();
await new Promise<void>((resolve) => reservation.listen(0, "127.0.0.1", resolve));
const reserved = reservation.address();
assert(reserved && typeof reserved !== "string");
await new Promise<void>((resolve, reject) => reservation.close((error) => error ? reject(error) : resolve()));
let server: Awaited<ReturnType<typeof createServer>> | undefined;
let browser: ChildProcess | undefined;
let exited: Promise<void> | undefined;
let startupError: Error | undefined;
let testError: unknown;
let finalReceipt: Record<string, unknown> | undefined;
let lifecycle: Record<string, unknown> | undefined;
let socket: WebSocket | undefined;
let nextId = 0;
const pending = new Map<number, { resolve: (value: any) => void; reject: (reason: unknown) => void; timer: ReturnType<typeof setTimeout> }>();
const errors: unknown[] = [];
const received: { id?: number; method?: string }[] = [];

async function command(method: string, params: Record<string, unknown> = {}): Promise<any> {
  assert(socket?.readyState === WebSocket.OPEN, "CDP socket is not open");
  const id = ++nextId;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}; received=${JSON.stringify(received.slice(-8))}`)); }, 10_000);
    pending.set(id, { resolve, reject, timer });
    socket!.send(JSON.stringify({ id, method, params }));
  });
}
async function evaluate(expression: string): Promise<any> {
  const result = await command("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  assert(!result.exceptionDetails, JSON.stringify(result.exceptionDetails));
  return result.result.value;
}
async function check(expression: string, message: string) {
  assert.equal(await evaluate(`Boolean(${expression})`), true, message);
}
async function click(row: string, label: string) {
  await evaluate(`(() => {
    const row = [...document.querySelectorAll('li')].find(row => row.querySelector('strong')?.textContent === ${JSON.stringify(row)});
    const button = [...(row?.querySelectorAll('button') ?? [])].find(button => button.textContent.trim() === ${JSON.stringify(label)});
    if (!button || button.disabled) throw new Error('Fixture button unavailable');
    button.click();
  })()`);
}
async function screenshot(name: string, width: number, height: number, mobile: boolean) {
  await command("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile });
  await evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
  await check("document.documentElement.scrollWidth <= innerWidth", "Web home overflows its viewport");
  const shot = await command("Page.captureScreenshot", { format: "png", captureBeyondViewport: true });
  await writeFile(`${output}/${name}.png`, Buffer.from(shot.data, "base64"));
}

const startupObservation = createStartupObservation((record) => {
  console.error("WEBHOME_STARTUP_OBSERVATION", JSON.stringify(record));
});

try {
  server = await createServer({
    configFile: `${root}vite.config.ts`, cacheDir: join(temporary, "vite-cache"),
    server: { host: "127.0.0.1", port: reserved.port, strictPort: true, hmr: false, open: false },
  });
  await server.listen();
  const address = server.httpServer!.address();
  assert(address && typeof address !== "string");
  const log = await open(`${output}/browser.log`, "wx", 0o600);
  browser = spawn("python3", [`${root}native-tests/browser/guard.py`, `${output}/guard.json`,
    process.env.CHROME_BIN || "/usr/bin/google-chrome", "--headless=new", "--no-sandbox",
    "--no-first-run", "--no-default-browser-check", "--disable-background-networking", "--disable-component-update",
    "--disable-sync", "--remote-debugging-port=0", `--user-data-dir=${temporary}/profile`,
    "--proxy-server=socks5://127.0.0.1:9", "--proxy-bypass-list=127.0.0.1;localhost;[::1]", "about:blank"],
    { stdio: ["ignore", "ignore", log.fd], detached: true });
  exited = new Promise<void>((resolve) => {
    browser!.once("exit", () => resolve());
    browser!.once("error", (error) => { startupError = error; resolve(); });
  });
  await log.close();
  const target = await initialPage(`${temporary}/profile/DevToolsActivePort`,
    () => guardRunning(browser!, startupError), { observe: startupObservation.observe });
  socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("Owned CDP handshake timed out")), 10_000);
    socket!.onopen = () => { clearTimeout(timer); resolve(); };
    socket!.onerror = () => { clearTimeout(timer); reject(new Error("Owned CDP handshake failed")); };
    socket!.onclose = () => { clearTimeout(timer); reject(new Error("Owned CDP closed before handshake")); };
  });
  socket.onmessage = (event) => {
    const message = JSON.parse(String(event.data));
    received.push({ id: message.id, method: message.method });
    if (received.length > 8) received.shift();
    if (message.id) {
      const operation = pending.get(message.id);
      if (operation) { clearTimeout(operation.timer); pending.delete(message.id); message.error ? operation.reject(message.error) : operation.resolve(message.result); }
    } else if (message.method === "Runtime.exceptionThrown") errors.push(message.params.exceptionDetails);
  };
  socket.onclose = () => { for (const operation of pending.values()) { clearTimeout(operation.timer); operation.reject(new Error("Owned CDP closed")); } pending.clear(); };
  const version = await command("Browser.getVersion");
  await command("Runtime.enable");
  await command("Page.enable");
  await command("Page.navigate", { url: `http://127.0.0.1:${address.port}/native-tests/browser/web-home.html` });
  const readyDeadline = Date.now() + 10_000;
  while (!await evaluate("typeof resetFixture !== 'undefined' && document.querySelectorAll('.web-connections li').length === 3")) {
    assert(Date.now() < readyDeadline, "Shared WebHome did not render");
    await delay(25); // Component readiness only; data verdicts below are once-only.
  }
  const initial = await evaluate("resetFixture.snapshot().saved");
  await check("[...document.querySelectorAll('button')].filter(b => b.textContent.trim() === '保存データを消去…').length === 2", "Shared legacy store was offered clear");
  await check("typeof window.__TAURI_INTERNALS__ === 'undefined'", "Fixture unexpectedly received native IPC");

  await click("Work", "保存データを消去…");
  await check("document.querySelector('.web-clear')?.getAttribute('aria-label') === 'Workの保存データ消去の確認'", "Wrong confirmation target");
  await check("[...document.querySelectorAll('form input,form button,.web-connection,.web-remove')].every(e => e.disabled)", "Other operations remain enabled during confirmation");
  await screenshot("desktop-confirm", 1280, 900, false);
  await screenshot("mobile-confirm", 390, 844, true);
  await click("Work", "キャンセル");
  await check("!document.querySelector('.web-clear') && resetFixture.snapshot().clears.length === 0", "Cancel triggered deletion");

  await evaluate("document.querySelector('.web-connection').click()");
  await check("document.querySelector('.web-connection small').textContent.includes('前の画面')", "Prior open control was not established");
  await click("Work", "保存データを消去…");
  await click("Work", "保存データを消去");
  await check("document.querySelector('.web-clear').textContent.includes('消去しています') && [...document.querySelectorAll('.web-clear button')].every(b => b.disabled)", "Pending clear does not lock controls");
  await check("!document.querySelector('[role=status]')", "Queued deletion was reported as success");
  const call = (await evaluate("resetFixture.snapshot().clears"))[0];
  assert.deepEqual(call, { command: "plugin:app-screen|clear_profile", args: { payload: {
    profile: "11111111-1111-4111-8111-111111111111", key: "web-home:11111111-1111-4111-8111-111111111111:https://sumi.example",
  } } });
  await evaluate("resetFixture.finish({cleared:false})");
  await check("document.querySelector('[role=alert]').textContent.includes('消去を確認') && !!document.querySelector('.web-clear')", "False receipt accepted or confirmation lost");
  await click("Work", "保存データを消去");
  await evaluate("resetFixture.finish({cleared:true})");
  await check("!document.querySelector('.web-clear') && document.querySelector('[role=status]').textContent === 'Workのサイトデータを消去しました。'", "Completed receipt did not update status");
  await check("!document.querySelector('.web-connection small').textContent.includes('前の画面')", "Cleared connection still offers old page");
  assert.deepEqual(await evaluate("resetFixture.snapshot().saved"), initial, "Clear removed connection settings");

  await click("Personal", "保存データを消去…");
  await click("Personal", "保存データを消去");
  await evaluate("resetFixture.fail('Synthetic native refusal')");
  await check("document.querySelector('[role=alert]').textContent === 'Synthetic native refusal' && !!document.querySelector('.web-clear')", "Native failure was hidden");
  await click("Personal", "キャンセル");
  await click("Work", "接続先から外す");
  const final = await evaluate("resetFixture.snapshot()");
  assert.equal(final.clears.length, 3, "Removing settings started another deletion");
  assert.deepEqual(final.saved, initial.slice(1), "Removing one setting changed another profile");

  const firstPrompt = { profile: initial[1].profile, promptId: 71, origin: "https://permission-first.example", kind: "prompt", permissions: 1 << 15 };
  const nextPrompt = { ...firstPrompt, promptId: 72, origin: "https://permission-next.example" };
  const permissionPanel = "document.querySelector('section[aria-labelledby=permissions-heading]')";
  await evaluate("resetFixture.permissions.begin()");
  await check("resetFixture.permissions.snapshot().listing", "Initial permission snapshot was not requested");
  await evaluate(`resetFixture.permissions.emit([${JSON.stringify(firstPrompt)}])`);
  await check(`${permissionPanel}?.textContent.includes('permission-first.example')`, "Live permission prompt did not render");
  await evaluate("resetFixture.permissions.finishList([])");
  await check(`${permissionPanel}?.textContent.includes('permission-first.example')`, "Late initial snapshot hid a newer permission prompt");

  await evaluate(`${permissionPanel}.querySelectorAll('button')[1].click()`);
  await check("resetFixture.permissions.snapshot().answering", "Permission answer did not reach its adapter");
  await evaluate(`resetFixture.permissions.emit([${JSON.stringify(nextPrompt)}])`);
  await evaluate("resetFixture.permissions.finishAnswer([])");
  await check(`${permissionPanel}?.textContent.includes('permission-next.example')`, "Late answer snapshot hid the next permission prompt");
  await evaluate(`${permissionPanel}.querySelector('button').click()`);
  await evaluate("resetFixture.permissions.failAnswer()");
  await check(`${permissionPanel}?.textContent.includes('permission-next.example') && document.querySelector('[role=alert]').textContent === 'Synthetic permission transport refusal'`, "Answer failure hid the held prompt or its cause");
  await evaluate(`${permissionPanel}.querySelector('button').click()`);
  await evaluate("resetFixture.permissions.finishAnswer([])");
  await check(`!${permissionPanel}`, "Successful answer did not retire its prompt");
  assert.deepEqual(await evaluate("resetFixture.permissions.snapshot().answers"), [
    { prompt: firstPrompt, decision: "allow" }, { prompt: nextPrompt, decision: "deny" }, { prompt: nextPrompt, decision: "deny" },
  ], "The shared UI answered the wrong held prompt or retried automatically");
  await evaluate("resetFixture.permissions.end()");
  await evaluate(`resetFixture.permissions.emit([${JSON.stringify(firstPrompt)}])`);
  await check(`!${permissionPanel}`, "An already queued callback revived a retired permission surface");

  await evaluate("resetFixture.permissions.begin()");
  await evaluate(`resetFixture.permissions.finishList([${JSON.stringify(nextPrompt)}])`);
  await check(`${permissionPanel}?.textContent.includes('permission-next.example')`, "Initial snapshot without a competing event was ignored");
  await evaluate("resetFixture.permissions.end()");
  await evaluate("resetFixture.permissions.begin()");
  await evaluate("resetFixture.permissions.end()");
  await evaluate(`resetFixture.permissions.finishList([${JSON.stringify(firstPrompt)}])`);
  await check(`!${permissionPanel}`, "A late snapshot revived a retired permission surface");
  await check("resetFixture.permissions.snapshot().unlistened === 3", "Permission registrations were not released exactly once");
  assert.equal(errors.length, 0, JSON.stringify(errors));
  assert.deepEqual(await sourceHashes(), before, "Fixture or shared source changed during the run");
  finalReceipt = { result: "pass", component: "shared WebHome + clear and permission adapters",
    native: "synthetic, no engine deletion or native permission", node: process.version, browser: version, sourceHashes: before,
    cases: ["shared clear hidden", "confirmation lock", "cancel", "pending", "false receipt", "completed receipt", "native refusal", "preference removal",
      "permission initial snapshot ordering", "permission answer ordering", "permission failure and explicit retry", "retired permission callbacks and snapshots"], final };
 } catch (error) {
  testError = error;
  startupObservation.failure(error);
  throw error;
} finally {
  let confirmed = !browser;
  try {
    if (socket?.readyState === WebSocket.OPEN) await command("Browser.close").catch(() => {});
    socket?.close();
    if (browser) {
      let ended = await Promise.race([exited!.then(() => true), delay(5_000).then(() => false)]);
      if (!ended) {
        browser.kill("SIGTERM"); // Only the owned guard, which reaps its browser tree.
        ended = await Promise.race([exited!.then(() => true), delay(5_000).then(() => false)]);
      }
      assert(ended && !startupError, "Browser guard did not finish; retain its private profile");
      const guard = confirmBrowserCleanup(browser.exitCode, JSON.parse(await readFile(`${output}/guard.json`, "utf8")), !testError);
      lifecycle = guard;
      confirmed = true;
    }
  } catch (cleanupError) {
    console.error("Owned browser cleanup:", cleanupError);
    if (!testError) throw cleanupError;
  } finally {
    await server?.close();
    // The path is a brand-new owned mkdtemp result. Failed lifecycle proof
    // preserves it, so no live browser can write into a deleted profile.
    if (confirmed) await rm(temporary, { recursive: true });
    else console.error(`Retained private browser directory: ${temporary}`);
  }
}
assert(finalReceipt && lifecycle, "No completed UI and browser lifecycle receipt");
await writeFile(`${output}/receipt.json`, JSON.stringify({ ...finalReceipt, lifecycle, privateProfileRemoved: true }, null, 2));
console.log("PASS: real shared WebHome clear and permission ordering/error/retirement; native adapters are simulated");
