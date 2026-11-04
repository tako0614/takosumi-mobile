import { expect, test } from "bun:test";
import { confirmBrowserCleanup } from "../native-tests/browser/lifecycle.ts";

const normal = { complete: true, kernelNoChildren: true, remaining: [], rootExit: 0, error: null, cleanupErrors: [] };

test("browser cleanup requires kernel proof and rejects incomplete or malformed receipts", () => {
  expect(confirmBrowserCleanup(0, normal, true)).toEqual(normal);
  for (const receipt of [undefined, null, {}, { ...normal, complete: "true" },
    { ...normal, kernelNoChildren: false }, { ...normal, remaining: [123] }, { ...normal, rootExit: null }]) {
    expect(() => confirmBrowserCleanup(0, receipt, true)).toThrow();
  }
});

test("a failed guard cannot pass merely because cleanup finished with root exit zero", () => {
  // An observer error can occur after the browser root exits normally. The
  // real fault control exercises this combination; complete is not success.
  const failed = { ...normal, error: "OSError" };
  expect(() => confirmBrowserCleanup(1, failed, true)).toThrow();
  expect(() => confirmBrowserCleanup(0, failed, true)).toThrow();
  expect(() => confirmBrowserCleanup(0, { ...normal, cleanupErrors: ["OSError"] }, true)).toThrow();
  expect(() => confirmBrowserCleanup(null, normal, true)).toThrow();
  // A failed UI run can safely remove an empty profile while propagating its
  // original failure. This result is never sufficient for a pass receipt.
  expect(confirmBrowserCleanup(1, failed, false)).toEqual(failed);
});
