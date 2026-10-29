import type { D1Database } from "./d1.ts";

export interface RateRule {
  /** What is being limited, for example `push-endpoint`. */
  scope: string;
  /** Who is being limited: an endpoint id, a device id, or a hashed address. */
  key: string;
  limit: number;
  windowSeconds: number;
}

export interface RateDecision {
  allowed: boolean;
  /** Seconds until the most restrictive exceeded window ends (0 when allowed). */
  retryAfter: number;
}

export interface RateLimiter {
  hit(rules: RateRule[], now: number): Promise<RateDecision>;
}

const UPSERT =
  "INSERT INTO rate_windows (bucket, window_start, count) VALUES (?1, ?2, 1) " +
  "ON CONFLICT (bucket) DO UPDATE SET " +
  "count = CASE WHEN rate_windows.window_start = excluded.window_start THEN rate_windows.count + 1 ELSE 1 END, " +
  "window_start = excluded.window_start " +
  "RETURNING count";

/**
 * Fixed-window counters in D1. Every rule is counted in one batch, so a push
 * costs one round trip for its limits. Counters are global (not per colo), which
 * is what a per-endpoint and per-device limit needs.
 */
export class D1RateLimiter implements RateLimiter {
  constructor(private readonly db: D1Database) {}

  async hit(rules: RateRule[], now: number): Promise<RateDecision> {
    if (rules.length === 0) return { allowed: true, retryAfter: 0 };
    const windows = rules.map((rule) => now - (now % rule.windowSeconds));
    const results = await this.db.batch<{ count: number }>(
      rules.map((rule, index) =>
        this.db.prepare(UPSERT).bind(`${rule.scope}:${rule.key}`, windows[index]),
      ),
    );
    let retryAfter = 0;
    rules.forEach((rule, index) => {
      const count = Number(results[index]?.results[0]?.count ?? 0);
      if (count > rule.limit) {
        const windowStart = windows[index] ?? now;
        retryAfter = Math.max(retryAfter, windowStart + rule.windowSeconds - now, 1);
      }
    });
    return { allowed: retryAfter === 0, retryAfter };
  }
}
