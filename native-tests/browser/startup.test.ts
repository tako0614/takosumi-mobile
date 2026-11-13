import assert from "node:assert/strict";
import test from "node:test";
import { createStartupObservation, guardRunning, initialPage } from "./startup.ts";

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


test("startup observation distinguishes header and body timeouts within the same attempt", async () => {
  for (const blocked of ["headers", "body"] as const) {
    const f = fixture();
    const records: Record<string, unknown>[] = [];
    const observation = createStartupObservation((record) => { records.push(record); }, f.elapsed);
    f.io.read = async () => { f.advance(16_000); return "9222\n"; };
    f.io.request = async (_port, timeout) => {
      f.calls.push(timeout);
      const error = new DOMException("private credential URL must never be logged", "TimeoutError");
      if (blocked === "headers") { f.advance(timeout); throw error; }
      f.advance(500);
      const reply = response();
      reply.json = async () => { f.advance(3_500); throw error; };
      return reply;
    };
    let cleanup = 0;
    await assert.rejects(async () => {
      try { await initialPage("unused", f.io.alive, { ...f.io, observe: observation.observe }); }
      catch (error) { observation.failure(error); throw error; }
      finally { cleanup += 1; f.advance(5_000); }
    }, /20000ms startup bound/);
    assert.deepEqual(f.calls, [4_000]);
    assert.equal(cleanup, 1);
    assert.equal(records.length, 1);
    const record = records[0]!;
    assert.equal(record.clockDomain, "host_unix_epoch");
    assert.equal(record.clockSource, "Date.now");
    assert.equal(record.clockUnit, "ms");
    assert.equal(record.observationError, false);
    assert.equal(record.startedAtMs, 0);
    assert.equal(record.deadlineAtMs, 20_000);
    assert.equal(record.portSeenAtMs, 16_000);
    assert.equal(record.failureBeforeFinallyAtMs, 20_000);
    assert.equal(record.failurePhase, blocked);
    const attempt = record.lastAttempt as Record<string, unknown>;
    assert.equal(attempt.attempt, 1);
    assert.equal(attempt.startedAtMs, 16_000);
    assert.equal(attempt.remainingMs, 4_000);
    assert.equal(attempt.timeoutMs, 4_000);
    assert.equal(attempt.errorAtMs, 20_000);
    assert.equal(attempt.errorPhase, blocked);
    assert.equal(attempt.errorCategory, "timeout");
    assert.equal(attempt.headersAtMs, blocked === "body" ? 16_500 : undefined);
    assert.equal(attempt.status, blocked === "body" ? 200 : undefined);
    assert.equal(attempt.bodyAtMs, undefined);
    assert(!JSON.stringify(record).includes("private credential"));
  }
});

test("startup observation preserves success and HTTP refusal with one bounded scalar summary", async () => {
  const success = fixture();
  const successRecords: Record<string, unknown>[] = [];
  const successful = createStartupObservation((record) => { successRecords.push(record); }, success.elapsed);
  assert.deepEqual(await initialPage("unused", success.io.alive, { ...success.io, observe: successful.observe }), target);
  successful.failure(new Error("a later UI failure is not a startup failure"));
  assert.deepEqual(successRecords, []);
  assert.deepEqual(success.calls, [5_000]);

  const refused = fixture();
  const records: Record<string, unknown>[] = [];
  const observation = createStartupObservation((record) => { records.push(record); }, refused.elapsed);
  refused.io.request = async (_port, timeout) => { refused.calls.push(timeout); return new Response("private body", { status: 503 }); };
  await assert.rejects(async () => {
    try { await initialPage("unused", refused.io.alive, { ...refused.io, observe: observation.observe }); }
    catch (error) { observation.failure(error); observation.failure(error); throw error; }
  }, /refused discovery/);
  assert.deepEqual(refused.calls, [5_000]);
  assert.equal(records.length, 1);
  assert.equal(records[0]!.failureCategory, "assertion");
  assert.equal(records[0]!.failurePhase, "http_status");
  assert.equal((records[0]!.lastAttempt as Record<string, unknown>).status, 503);
  assert(!JSON.stringify(records).includes("private body"));

  const bounded: Record<string, unknown>[] = [];
  const summary = createStartupObservation((record) => { bounded.push(record); }, () => 17);
  summary.observe({ stage: "start", deadlineAtMs: 20_000 });
  for (let attempt = 1; attempt <= 1_000; attempt += 1) {
    summary.observe({ stage: "request", attempt, remainingMs: 1, timeoutMs: 1 });
    summary.observe({ stage: "error", phase: "headers", category: "timeout" });
  }
  summary.failure(new Error("raw error is private"));
  assert.equal(bounded.length, 1);
  assert.equal((bounded[0]!.lastAttempt as Record<string, unknown>).attempt, 1_000);
  assert(Buffer.byteLength(JSON.stringify(bounded[0])) < 1_024);
  assert(!JSON.stringify(bounded).includes("raw error"));
});

test("startup observation failures preserve the original exception and caller cleanup", async () => {
  for (const broken of ["observer", "clock", "sink"] as const) {
    const f = fixture();
    const original = Object.assign(new Error("original private failure"), { code: "EACCES" });
    f.io.read = async () => { throw original; };
    let cleanup = 0, emits = 0;
    const records: Record<string, unknown>[] = [];
    const observation = createStartupObservation((record) => {
      emits += 1;
      if (broken === "sink") throw new Error("observation output unavailable");
      records.push(record);
    }, broken === "clock" ? () => { throw new Error("observation clock unavailable"); } : f.elapsed);
    await assert.rejects(async () => {
      try {
        await initialPage("unused", f.io.alive, { ...f.io, observe: broken === "observer"
          ? () => { throw new Error("observation unavailable"); } : observation.observe });
      } catch (error) { observation.failure(error); throw error; }
      finally { cleanup += 1; }
    }, (error: unknown) => error === original);
    assert.equal(cleanup, 1);
    assert.deepEqual(f.calls, []);
    assert.equal(f.elapsed(), 0);
    assert.equal(emits, broken === "observer" ? 0 : 1);
    if (broken === "clock") {
      assert.equal(records[0]!.observationError, true);
      assert.equal(records[0]!.failureBeforeFinallyAtMs, undefined);
    }
  }
});
