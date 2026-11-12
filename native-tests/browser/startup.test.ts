import assert from "node:assert/strict";
import test from "node:test";
import { guardRunning, initialPage } from "./startup.ts";

const target = { type: "page", url: "about:blank", webSocketDebuggerUrl: "ws://127.0.0.1:9222/devtools/page/owned" };
const response = (targets: unknown = [target]) => new Response(JSON.stringify(targets));
function fixture() {
  let clock = 0;
  let live = true;
  const calls: number[] = [];
  const io = {
    alive: () => live,
    now: () => clock,
    delay: async (milliseconds: number) => { clock += milliseconds; },
    read: async () => "9222\n/devtools/browser/owned\n",
    request: async (port: number, timeout: number) => {
      assert.equal(port, 9222);
      calls.push(timeout);
      return response();
    },
  };
  return { io, calls, advance: (milliseconds: number) => { clock += milliseconds; },
    stop: () => { live = false; }, elapsed: () => clock };
}

test("the private file may precede an HTTP service without restarting the owned browser", async () => {
  const f = fixture();
  let reads = 0;
  let requests = 0;
  f.io.read = async () => {
    reads += 1;
    if (reads < 3) throw Object.assign(new Error(), { code: "ENOENT" });
    return "9222\n/devtools/browser/owned\n";
  };
  f.io.request = async (port, timeout) => {
    assert.equal(port, 9222);
    f.calls.push(timeout);
    requests += 1;
    if (requests === 1) throw new TypeError("fetch failed");
    return response();
  };
  assert.deepEqual(await initialPage("unused", f.io.alive, f.io), target);
  assert.equal(reads, 3);
  assert.equal(requests, 2);
  assert.equal(f.elapsed(), 75);
});

test("an HTTP timeout uses the remainder of the same 20-second startup budget", async () => {
  const f = fixture();
  f.advance(0);
  f.io.read = async () => { f.advance(16_000); return "9222\n"; };
  f.io.request = async (_port, timeout) => {
    f.calls.push(timeout);
    if (f.calls.length === 1) {
      f.advance(3_000);
      throw new DOMException("network not ready", "TimeoutError");
    }
    return response();
  };
  assert.deepEqual(await initialPage("unused", f.io.alive, f.io), target);
  assert.deepEqual(f.calls, [4_000, 975]);
  assert.equal(f.elapsed(), 19_025);
});

test("persistent network unavailability exhausts the original deadline", async () => {
  const f = fixture();
  f.io.request = async (_port, timeout) => {
    f.calls.push(timeout);
    f.advance(timeout);
    throw new DOMException("network not ready", "TimeoutError");
  };
  await assert.rejects(initialPage("unused", f.io.alive, f.io), /20000ms startup bound/);
  assert.deepEqual(f.calls, [5_000, 5_000, 5_000, 4_925]);
  assert.equal(f.elapsed(), 20_000);
});

test("a missing private file reaches the bound without contacting a port", async () => {
  const f = fixture();
  f.io.read = async () => { throw Object.assign(new Error(), { code: "ENOENT" }); };
  await assert.rejects(initialPage("unused", f.io.alive, f.io), /20000ms startup bound/);
  assert.deepEqual(f.calls, []);
  assert.equal(f.elapsed(), 20_000);
});

test("an ended owner refuses discovery before or after the response", async () => {
  const before = fixture();
  before.stop();
  await assert.rejects(initialPage("unused", before.io.alive, before.io), /exited during startup/);
  assert.deepEqual(before.calls, []);
  const after = fixture();
  after.io.request = async () => { after.stop(); return response(); };
  await assert.rejects(initialPage("unused", after.io.alive, after.io), /exited during discovery/);
});

test("the caller refuses normal exit, signal exit and spawn failure", () => {
  assert.equal(guardRunning({ exitCode: null, signalCode: null }), true);
  assert.equal(guardRunning({ exitCode: 0, signalCode: null }), false);
  assert.equal(guardRunning({ exitCode: null, signalCode: "SIGKILL" }), false);
  assert.equal(guardRunning({ exitCode: null, signalCode: null }, new Error("spawn failed")), false);
});

test("an owner ending during the private file read cannot authorize an HTTP request", async () => {
  const f = fixture();
  f.io.read = async () => { f.stop(); return "9222\n"; };
  await assert.rejects(initialPage("unused", f.io.alive, f.io), /exited before endpoint discovery/);
  assert.deepEqual(f.calls, []);
});

test("a response beyond the startup deadline cannot qualify readiness", async () => {
  const f = fixture();
  f.io.request = async () => { f.advance(20_001); return response(); };
  await assert.rejects(initialPage("unused", f.io.alive, f.io), /after the startup deadline/);
});

test("port-file errors and malformed ports are refusals, not readiness retries", async () => {
  for (const text of ["-1\n", "0\n", "65536\n", "9.5\n", "9222anything\n"]) {
    const f = fixture();
    f.io.read = async () => text;
    await assert.rejects(initialPage("unused", f.io.alive, f.io), /private CDP port/);
    assert.deepEqual(f.calls, []);
    assert.equal(f.elapsed(), 0);
  }
  const f = fixture();
  f.io.read = async () => { throw Object.assign(new Error("denied"), { code: "EACCES" }); };
  await assert.rejects(initialPage("unused", f.io.alive, f.io), /denied/);
  assert.equal(f.elapsed(), 0);
});

test("HTTP refusal, malformed JSON and invalid target lists fail on their first response", async () => {
  for (const reply of [
    () => new Response("not ready", { status: 503 }),
    () => new Response("malformed JSON"),
    () => response({}), () => response([]), () => response([target, target]),
    () => response([{ ...target, url: "https://unexpected.example" }]),
    () => response([{ ...target, webSocketDebuggerUrl: "ws://127.0.0.1:9223/devtools/page/foreign" }]),
  ]) {
    const f = fixture();
    let calls = 0;
    f.io.request = async () => { calls += 1; return reply(); };
    await assert.rejects(initialPage("unused", f.io.alive, f.io));
    assert.equal(calls, 1);
    assert.equal(f.elapsed(), 0);
  }
});
