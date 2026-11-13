// Live proof for the browser-process permission pipe: one profile's real
// Notification.requestPermission is held on the wire, answered by the
// parent's resolvePermission, and reads back granted through the runtime —
// while a second profile gets denied and stays denied. Driven through a
// local Xvfb session. This is a fixture driver, never product behavior: the
// opt-in env is what the App will set only once it owns a real prompt
// surface.
import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import path from 'node:path';

const [data, binary] = process.argv.slice(2);
assert(data && path.isAbsolute(data) && binary && path.isAbsolute(binary));
const nonce = (await readFile(path.join(data, '.owned-browser-reference'), 'utf8')).trim();
const env = { ...Object.fromEntries(Object.entries(process.env).filter(([key]) =>
  ['PATH', 'USER', 'LOGNAME', 'LANG', 'LC_ALL', 'LC_CTYPE', 'DISPLAY', 'XAUTHORITY',
   'DBUS_SESSION_BUS_ADDRESS'].includes(key))),
  PYTHONDONTWRITEBYTECODE: '1', LD_LIBRARY_PATH: path.dirname(binary) };
const receipt = { engine: 'pipe browser-process', nonce, profiles: [] };

class PipeBrowser {
  constructor(profile) {
    this.profile = profile;
    this.dir = path.join(data, profile.slice(0, 1));
    this.next = 1;
    this.lines = [];
  }
  async start(url) {
    const dir = this.dir;
    for (const name of ['Downloads', 'home', 'data', 'config', 'cache', 'runtime', 'tmp'])
      await mkdir(path.join(dir, name), { mode: 0o700, recursive: true });
    const boot = { operation: 'open', id: 1, payload: {
      key: `web-home:${this.profile}:${new URL(url).origin}`, url, title: 'Pipe permission',
      profile: this.profile, webHome: true, resume: false } };
    await writeFile(path.join(dir, 'boot.json'), JSON.stringify(boot));
    const childEnv = { ...env,
      HOME: path.join(dir, 'home'), XDG_DATA_HOME: path.join(dir, 'data'),
      XDG_CONFIG_HOME: path.join(dir, 'config'), XDG_CACHE_HOME: path.join(dir, 'cache'),
      XDG_RUNTIME_DIR: path.join(dir, 'runtime'), TMPDIR: path.join(dir, 'tmp'),
      TAKOSUMI_BROWSER_PROCESS_DIRECTORY: dir,
      TAKOSUMI_BROWSER_PROCESS_INSPECT: '1',
      TAKOSUMI_BROWSER_PROCESS_PERMISSION_EVENTS: '1' };
    const log = (await import('node:fs')).openSync(path.join(dir, 'browser.log'), 'w', 0o600);
    this.child = spawn(binary, [
      '--no-sandbox', '--no-first-run', '--no-default-browser-check',
      '--disable-background-networking', '--disable-component-update',
      '--disable-sync', '--password-store=basic', '--lang=en-US'],
      { env: childEnv, stdio: ['pipe', 'pipe', log] });
    this.child.stdout.setEncoding('utf8');
    let buffer = '';
    this.child.stdout.on('data', chunk => {
      buffer += chunk;
      for (let i = buffer.indexOf('\n'); i >= 0; i = buffer.indexOf('\n')) {
        const line = buffer.slice(0, i); buffer = buffer.slice(i + 1);
        this.lines.push(JSON.parse(line));
      }
    });
    const opened = await this.event(reply => reply.event === 'opened', 30000);
    assert.equal(opened.profile, this.profile);
    this.nativePid = opened.native_pid;
    const inspect = JSON.parse(await readFile(path.join(dir, 'inspection.json'), 'utf8'));
    assert.equal(inspect.pid, this.nativePid, 'The inspection endpoint must be the attested child');
    // The first real main-frame load finishes the secure context; the
    // fixture-inspection LoadHandler marks it in the profile directory.
    const end = Date.now() + 30000;
    let ready = false;
    while (Date.now() < end && !ready) {
      ready = await import('node:fs/promises')
        .then(fs => fs.access(path.join(dir, `load-ready-${opened.browser_id}.json`)))
        .then(() => true, () => false);
      if (!ready) await delay(100);
    }
    assert(ready, 'The first page load did not complete');
  }
  async event(match, timeout) {
    const end = Date.now() + timeout;
    while (Date.now() < end) {
      const found = this.lines.find(match);
      if (found) return found;
      assert.equal(this.child.exitCode, null, 'The browser ended before the expected event');
      await delay(25);
    }
    throw Error(`Pipe event timeout; saw ${JSON.stringify(this.lines)}`);
  }
  send(packet) { this.child.stdin.write(JSON.stringify(packet) + '\n'); }
  async cdp() {
    const inspect = JSON.parse(await readFile(path.join(this.dir, 'inspection.json'), 'utf8'));
    const targets = await (await fetch(`http://127.0.0.1:${inspect.port}/json/list`,
      { signal: AbortSignal.timeout(5000) })).json();
    const page = targets.find(target => target.type === 'page');
    const socket = new WebSocket(page.webSocketDebuggerUrl);
    await new Promise((resolve, reject) => {
      socket.onopen = resolve;
      socket.onerror = () => reject(Error('CDP socket failed'));
      setTimeout(() => reject(Error('CDP socket timeout')), 5000);
    });
    let next = 0; const pending = new Map();
    socket.onmessage = event => {
      const message = JSON.parse(String(event.data));
      const item = pending.get(message.id);
      if (!item) return;
      clearTimeout(item.timer); pending.delete(message.id);
      if (message.error) item.reject(Error(JSON.stringify(message.error)));
      else item.resolve(message.result);
    };
    const send = (method, params = {}) => new Promise((resolve, reject) => {
      const id = ++next;
      pending.set(id, { resolve, reject, timer: setTimeout(() => reject(Error(`CDP timeout ${method}`)), 10000) });
      socket.send(JSON.stringify({ id, method, params }));
    });
    return { send, close: () => socket.close() };
  }
  async evaluate(cdp, expression) {
    const result = await cdp.send('Runtime.evaluate',
      { expression, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) throw Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  }
  async click(cdp) {
    for (const type of ['mousePressed', 'mouseReleased'])
      await cdp.send('Input.dispatchMouseEvent', { type, x: 320, y: 240, button: 'left', clickCount: 1 });
  }
}

const URL_SITE = 'https://excalidraw.com/';
const [work, personal] = [
  '11111111-1111-4111-8111-111111111111',
  '22222222-2222-4222-8222-222222222222'];

for (const [profile, decision] of [[work, 'allow'], [personal, 'deny']]) {
  const browser = new PipeBrowser(profile);
  await browser.start(URL_SITE);
  const cdp = await browser.cdp();
  await browser.evaluate(cdp, 'document.readyState');
  const context = await browser.evaluate(cdp,
    '({origin: location.origin, secure: isSecureContext, permission: Notification.permission})');
  assert.equal(context.origin, new URL(URL_SITE).origin, 'The page must be the bound origin');
  assert.equal(context.secure, true, 'Permission requests need a secure context');
  assert.equal(context.permission, 'default', 'A fresh profile starts undecided');
  // A real input event supplies the user activation requestPermission wants.
  await browser.click(cdp);
  const marker = `sumi_permission_${nonce.replaceAll('-', '')}`;
  const started = await browser.evaluate(cdp,
    `(() => { globalThis[${JSON.stringify(marker)}] = Notification.requestPermission();
      globalThis[${JSON.stringify(marker)}].then(v => globalThis[${JSON.stringify(marker + '_result')}] = v);
      return Notification.permission; })()`);
  assert.equal(started, 'default', 'The request must start undecided');
  // The held request reaches the pipe exactly once, naming this profile.
  const asked = await browser.event(
    reply => reply.event === 'permissionRequested' && reply.profile === browser.profile, 20000);
  assert.equal(asked.kind, 'prompt');
  assert.equal(new URL(asked.origin).origin, new URL(URL_SITE).origin);
  assert.notEqual(asked.prompt_id, 0);
  browser.send({ operation: 'resolvePermission', id: ++browser.next, prompt_id: asked.prompt_id, decision });
  await browser.event(reply => reply.event === 'permissionResolved' && reply.id === browser.next, 15000);
  const expected = decision === 'allow' ? 'granted' : 'denied';
  const end = Date.now() + 15000; let resolved = null;
  while (Date.now() < end && resolved !== expected) {
    resolved = await browser.evaluate(cdp, `globalThis[${JSON.stringify(marker + '_result')}] ?? 'pending'`);
    if (resolved === 'pending') await delay(100);
  }
  assert.equal(resolved, expected,
    `resolvePermission(${decision}) must apply the real runtime grant`);
  assert.equal(await browser.evaluate(cdp, 'Notification.permission'), expected);
  receipt.profiles.push({ profile, nativePid: browser.nativePid, promptId: asked.prompt_id,
    kind: asked.kind, origin: asked.origin, decision, result: resolved });
  cdp.close();
  browser.send({ operation: 'shutdown', id: ++browser.next });
  await browser.event(reply => reply.event === 'stopped', 30000);
  browser.child.stdin.end();
  await Promise.race([new Promise(r => browser.child.once('exit', r)), delay(15000)]);
  assert.equal(browser.child.exitCode, 0, 'The browser must end normally after a resolved session');
}
await writeFile(path.join(data, 'pipe-permission.json'), JSON.stringify(receipt, null, 2) + '\n');
console.log('pipe-permission: PASS', JSON.stringify(receipt.profiles));
