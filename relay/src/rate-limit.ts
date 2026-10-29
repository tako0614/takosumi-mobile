import type { D1Database } from "./d1.ts";

/** A fixed-window counter: `bucket` counts events in windows of `windowSeconds`. */
export interface WindowCounter {
  bucket: string;
  windowSeconds: number;
}

export interface CounterOp {
  counter: WindowCounter;
  /** true adds one and returns the new count; false only reads the count. */
  increment: boolean;
}

/**
 * Fixed-window counters. Counts are global (not per colo), which is what
 * per-endpoint, per-device, per-address and breaker counts need.
 */
export interface Counters {
  /** Runs every op in one round trip and returns the counts in order. */
  run(ops: CounterOp[], now: number): Promise<number[]>;
}

export interface RateRule {
  /** What is being limited, for example `register`. */
  scope: string;
  /** Who is being limited: a device id or a hashed address. */
  key: string;
  limit: number;
  windowSeconds: number;
}

export interface RateDecision {
  allowed: boolean;
  /** Seconds until the window ends (0 when allowed). */
  retryAfter: number;
}

export function windowStart(now: number, windowSeconds: number): number {
  return now - (now % windowSeconds);
}

export function secondsLeft(now: number, windowSeconds: number): number {
  return Math.max(windowStart(now, windowSeconds) + windowSeconds - now, 1);
}

/** Counts one event against `rule` and says whether it is within the limit. */
export async function limit(counters: Counters, rule: RateRule, now: number): Promise<RateDecision> {
  const [count = 0] = await counters.run(
    [{ counter: { bucket: `${rule.scope}:${rule.key}`, windowSeconds: rule.windowSeconds }, increment: true }],
    now,
  );
  return count > rule.limit
    ? { allowed: false, retryAfter: secondsLeft(now, rule.windowSeconds) }
    : { allowed: true, retryAfter: 0 };
}

const UPSERT =
  "INSERT INTO rate_windows (bucket, window_start, count) VALUES (?1, ?2, 1) " +
  "ON CONFLICT (bucket) DO UPDATE SET " +
  "count = CASE WHEN rate_windows.window_start = excluded.window_start THEN rate_windows.count + 1 ELSE 1 END, " +
  "window_start = excluded.window_start " +
  "RETURNING count";
const PEEK = "SELECT count FROM rate_windows WHERE bucket = ?1 AND window_start = ?2";

export class D1Counters implements Counters {
  constructor(private readonly db: D1Database) {}

  async run(ops: CounterOp[], now: number): Promise<number[]> {
    if (ops.length === 0) return [];
    const results = await this.db.batch<{ count: number }>(
      ops.map((op) =>
        this.db
          .prepare(op.increment ? UPSERT : PEEK)
          .bind(op.counter.bucket, windowStart(now, op.counter.windowSeconds)),
      ),
    );
    return ops.map((_, index) => Number(results[index]?.results[0]?.count ?? 0));
  }
}
