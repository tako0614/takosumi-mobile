/** Private fixture hints, never a rendering, account or readiness verdict. */
export const BOOTSTRAP_EVENT = "takosumi-fixture-bootstrap";
export const BOOTSTRAP_STAGES = [
  "entry", "storage_begin", "storage_end", "storage_unavailable", "selection_begin", "selection_end",
  "legacy", "home_init", "connections_begin", "connections_end", "connections_failed", "save_element",
  "render_returned",
] as const;
export type BootstrapStage = typeof BOOTSTRAP_STAGES[number];
type Packet = readonly [1, number, BootstrapStage | "cap", number | null];

/** Calls neither the sink nor the browser clock when the fixture flag is off. */
export function createBootstrapObserver(input: {
  readonly enabled: boolean;
  readonly send: (packet: Packet) => unknown;
  readonly now: () => unknown;
}): (stage: BootstrapStage) => void {
  let sequence = 0;
  return (stage) => {
    if (!input.enabled || !BOOTSTRAP_STAGES.includes(stage) || sequence > 16) return;
    sequence += 1;
    const capped = sequence === 17;
    let elapsed: number | null = null;
    if (!capped) {
      try {
        const value = input.now();
        if (typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 600_000) {
          elapsed = Math.floor(value);
        }
      } catch { /* Unsupported/failed clocks stay unknown. */ }
    }
    try {
      // Fire and forget on the existing event bus; never delay the bootstrap.
      void Promise.resolve(input.send([1, capped ? 0 : sequence, capped ? "cap" : stage, elapsed]))
        .catch(() => {});
    } catch { /* A diagnostic must not replace the bootstrap's result or exception. */ }
  };
}
