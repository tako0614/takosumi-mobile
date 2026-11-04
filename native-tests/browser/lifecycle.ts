import assert from "node:assert/strict";

/** Cleanup evidence is distinct from successful test execution. */
export function confirmBrowserCleanup(exitCode: number | null, value: unknown, requireSuccess: boolean): Record<string, unknown> {
  assert(value !== null && typeof value === "object", "Missing browser lifecycle receipt");
  const receipt = value as Record<string, unknown>;
  assert(receipt.complete === true && receipt.kernelNoChildren === true &&
    Array.isArray(receipt.remaining) && receipt.remaining.length === 0 &&
    typeof receipt.rootExit === "number" && Number.isInteger(receipt.rootExit),
  "Owned browser helpers may remain; retain their profile");
  if (requireSuccess) {
    assert.equal(exitCode, 0, "Owned browser guard failed");
    assert.equal(receipt.rootExit, 0, "Owned browser did not exit normally");
    assert.equal(receipt.error, null, "Browser observation failed");
    assert.deepEqual(receipt.cleanupErrors, [], "Browser cleanup encountered an error");
  }
  return receipt;
}
