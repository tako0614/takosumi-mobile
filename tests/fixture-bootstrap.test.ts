import { describe, expect, test } from "bun:test";
import { BOOTSTRAP_STAGES, createBootstrapObserver } from "../src/fixture-bootstrap.ts";
import { loadWebConnections, webClientSelected } from "../src/web-connections.ts";

describe("private root bootstrap hints", () => {
  test("disabled recording never invokes the clock or existing bus", () => {
    let calls = 0;
    const note = createBootstrapObserver({ enabled: false, send: () => { calls++; }, now: () => { calls++; } });
    for (const stage of BOOTSTRAP_STAGES) note(stage);
    expect(calls).toBe(0);
  });

  test("only closed stages and finite document-relative time cross the bus", () => {
    const packets: unknown[] = [];
    let clock: unknown = 12.9;
    const note = createBootstrapObserver({ enabled: true, send: (p) => { packets.push(p); }, now: () => clock });
    note("entry");
    for (clock of [undefined, NaN, Infinity, -1, 600001, "SECRET"]) note("selection_begin");
    note("SECRET" as never);
    expect(packets).toEqual([[1, 1, "entry", 12], ...Array.from({ length: 6 }, (_, i) => [1, i + 2, "selection_begin", null])]);
    for (let i = 0; i < 30; i++) note("save_element");
    expect(packets).toHaveLength(17);
    expect(packets[16]).toEqual([1, 0, "cap", null]);
    expect(JSON.stringify(packets)).not.toContain("SECRET");
    expect(JSON.stringify(packets)).not.toContain("ready");
  });

  test("failed or rejected diagnostics preserve storage behavior and the original failure", async () => {
    for (const send of [() => { throw new Error("SECRET"); }, () => Promise.reject(new Error("SECRET"))]) {
      const reads: string[] = [];
      const storage = { getItem: (key: string) => { reads.push(key); return null; }, setItem: () => {} };
      const run = (observed: boolean) => {
        const note = createBootstrapObserver({ enabled: observed, send, now: () => { throw new Error("SECRET"); } });
        note("entry");
        const selected = webClientSelected("", storage);
        note("selection_end");
        const connections = loadWebConnections(storage);
        note("save_element");
        return { selected, connections };
      };
      const before = run(false);
      const oldReads = [...reads];
      reads.length = 0;
      expect(run(true)).toEqual(before);
      expect(reads).toEqual(oldReads);
      const original = new Error("original bootstrap failure");
      const note = createBootstrapObserver({ enabled: true, send, now: () => 0 });
      let caught: unknown;
      try { note("entry"); throw original; } catch (error) { caught = error; }
      expect(caught).toBe(original);
    }
    await Promise.resolve();
  });
});
