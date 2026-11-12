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
};

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
      try {
        response = await io.request(port, Math.min(5_000, remaining));
        assert(response.ok, "Private CDP endpoint refused discovery");
        targets = await response.json() as Target[];
      } catch (error) {
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
      return target;
    }
    await io.delay(Math.min(25, Math.max(0, deadline - io.now())));
  }
  throw new Error(`Owned Chrome discovery exceeded its 20000ms startup bound; attempts=${attempts}; last=${lastError}`);
}
