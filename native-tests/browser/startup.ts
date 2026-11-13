// The private port file can precede a responsive CDP service. Discovery and
// HTTP readiness share one startup budget; page/data verdicts are never retried.
import assert from "node:assert/strict";
import type { ChildProcess } from "node:child_process";
import { readFile } from "node:fs/promises";
import { setTimeout as pause } from "node:timers/promises";

type Target = { type: string; url: string; webSocketDebuggerUrl: string };
type Discovery = {
  alive: () => boolean;
  read: () => Promise<string>;
  request: (port: number, timeout: number) => Promise<Response>;
  now: () => number;
  delay: (milliseconds: number) => Promise<void>;
  observe?: (event: StartupEvent) => void;
};

type StartupPhase = "port_file" | "headers" | "http_status" | "body" | "page_validation";
type ErrorCategory = "timeout" | "abort" | "type" | "syntax" | "assertion" | "other";
type StartupEvent =
  | { stage: "start"; deadlineAtMs: number }
  | { stage: "port" }
  | { stage: "request"; attempt: number; remainingMs: number; timeoutMs: number }
  | { stage: "headers"; status: number }
  | { stage: "body_start" | "body_end" | "ready" }
  | { stage: "error"; phase: StartupPhase; category: ErrorCategory };
type AttemptObservation = {
  attempt?: number; startedAtMs?: number; remainingMs?: number; timeoutMs?: number;
  headersAtMs?: number; status?: number; bodyAtMs?: number;
  errorAtMs?: number; errorPhase?: StartupPhase; errorCategory?: ErrorCategory;
};

function errorCategory(error: unknown): ErrorCategory {
  if (error instanceof DOMException && error.name === "TimeoutError") return "timeout";
  if (error instanceof DOMException && error.name === "AbortError") return "abort";
  if (error instanceof TypeError) return "type";
  if (error instanceof SyntaxError) return "syntax";
  if (error instanceof assert.AssertionError) return "assertion";
  return "other";
}

/** One fixed-size summary; observation and its sink never determine readiness. */
export function createStartupObservation(emit: (record: Record<string, unknown>) => void, now = Date.now) {
  let started = false, ready = false, emitted = false, observationError = false;
  let phase: StartupPhase = "port_file";
  let startedAtMs: number | undefined, deadlineAtMs: number | undefined, portSeenAtMs: number | undefined;
  let lastAttempt: AttemptObservation | undefined;
  const scalar = (value: unknown, maximum = Number.MAX_SAFE_INTEGER): number | undefined => {
    if (typeof value === "number" && Number.isSafeInteger(value) && value >= 0 && value <= maximum) return value;
    observationError = true;
  };
  const time = (): number | undefined => {
    try { return scalar(now()); } catch { observationError = true; }
  };
  return {
    observe(event: StartupEvent): void {
      try {
        switch (event.stage) {
          case "start":
            started = true; deadlineAtMs = scalar(event.deadlineAtMs);
            startedAtMs = deadlineAtMs === undefined ? undefined : scalar(deadlineAtMs - 20_000);
            break;
          case "port": portSeenAtMs ??= time(); break;
          case "request":
            phase = "headers";
            lastAttempt = { attempt: scalar(event.attempt), startedAtMs: time(),
              remainingMs: scalar(event.remainingMs, 20_000), timeoutMs: scalar(event.timeoutMs, 5_000) };
            break;
          case "headers":
            phase = "http_status";
            if (lastAttempt) { lastAttempt.headersAtMs = time(); lastAttempt.status = scalar(event.status, 599); }
            break;
          case "body_start": phase = "body"; break;
          case "body_end":
            phase = "page_validation";
            if (lastAttempt) lastAttempt.bodyAtMs = time();
            break;
          case "error":
            if (lastAttempt) {
              lastAttempt.errorAtMs = time(); lastAttempt.errorPhase = event.phase; lastAttempt.errorCategory = event.category;
            }
            break;
          case "ready": ready = true; break;
        }
      } catch { observationError = true; }
    },
    /** Called by the owning catch before finally starts, never after cleanup. */
    failure(error: unknown): void {
      try {
        if (!started || ready || emitted) return;
        emitted = true;
        const failureBeforeFinallyAtMs = time();
        emit({ kind: "takosumi.browser-startup-observation@v1", clockDomain: "host_unix_epoch",
          clockSource: "Date.now", clockUnit: "ms", observationError, startedAtMs, deadlineAtMs, portSeenAtMs,
          lastAttempt: lastAttempt ? { ...lastAttempt } : undefined,
          failurePhase: phase, failureCategory: errorCategory(error), failureBeforeFinallyAtMs });
      } catch { /* Metadata must never replace the original failure or its cleanup. */ }
    },
  };
}

function observeStartup(io: Discovery, event: () => StartupEvent): void {
  try { io.observe?.(event()); } catch { /* Optional observation is non-authoritative. */ }
}

export function guardRunning(guard: Pick<ChildProcess, "exitCode" | "signalCode">, startupError?: Error): boolean {
  return !startupError && guard.exitCode === null && guard.signalCode === null;
}

export async function initialPage(portFile: string, alive: () => boolean, hooks: Partial<Discovery> = {}) {
  const io: Discovery = {
    alive, read: () => readFile(portFile, "utf8"),
    request: (port, timeout) => fetch(`http://127.0.0.1:${port}/json/list`, { signal: AbortSignal.timeout(timeout) }),
    now: Date.now, delay: pause, ...hooks,
  };
  const deadline = io.now() + 20_000;
  observeStartup(io, () => ({ stage: "start", deadlineAtMs: deadline }));
  let port = 0;
  let attempts = 0;
  let lastError = "port-unavailable";
  while (io.now() < deadline) {
    assert(io.alive(), "Owned Chrome exited during startup");
    if (!port) {
      try {
        const text = (await io.read()).split("\n")[0]!;
        if (text) {
          assert(/^[0-9]+$/.test(text), "Malformed private CDP port");
          port = Number(text);
          assert(Number.isInteger(port) && port > 0 && port <= 65535, "Invalid private CDP port");
          observeStartup(io, () => ({ stage: "port" }));
        }
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
      }
    }
    if (port && io.now() < deadline) {
      assert(io.alive(), "Owned Chrome exited before endpoint discovery");
      const remaining = deadline - io.now();
      if (remaining <= 0) break;
      attempts += 1;
      let response: Response;
      let targets: Target[];
      let phase: StartupPhase = "headers";
      observeStartup(io, () => ({ stage: "request", attempt: attempts, remainingMs: remaining, timeoutMs: Math.min(5_000, remaining) }));
      try {
        response = await io.request(port, Math.min(5_000, remaining));
        phase = "http_status";
        observeStartup(io, () => ({ stage: "headers", status: response.status }));
        assert(response.ok, "Private CDP endpoint refused discovery");
        phase = "body";
        observeStartup(io, () => ({ stage: "body_start" }));
        targets = await response.json() as Target[];
        observeStartup(io, () => ({ stage: "body_end" }));
      } catch (error) {
        observeStartup(io, () => ({ stage: "error", phase, category: errorCategory(error) }));
        // Fetch connection failures and aborted network/body reads are startup
        // readiness failures. HTTP/schema/page assertions remain once-only.
        if (!(error instanceof TypeError) && !(error instanceof DOMException &&
          (error.name === "TimeoutError" || error.name === "AbortError"))) throw error;
        lastError = error instanceof TypeError ? "TypeError" : error.name;
        await io.delay(Math.min(25, Math.max(0, deadline - io.now())));
        continue;
      }
      assert(io.alive(), "Owned Chrome exited during discovery");
      assert(io.now() <= deadline, "Private CDP response arrived after the startup deadline");
      assert(Array.isArray(targets), "Private CDP target list is malformed");
      const pages = targets.filter((target) => target.type === "page");
      assert.equal(pages.length, 1, "Fresh owned browser must have exactly its initial page");
      const target = pages[0]!;
      assert.equal(target.url, "about:blank", "Unexpected page in fresh owned browser");
      assert(typeof target.webSocketDebuggerUrl === "string" &&
        target.webSocketDebuggerUrl.startsWith(`ws://127.0.0.1:${port}/`));
      observeStartup(io, () => ({ stage: "ready" }));
      return target;
    }
    await io.delay(Math.min(25, Math.max(0, deadline - io.now())));
  }
  throw new Error(`Owned Chrome discovery exceeded its 20000ms startup bound; attempts=${attempts}; last=${lastError}`);
}
