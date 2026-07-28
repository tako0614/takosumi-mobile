import { expect, test } from "bun:test";
import type { NativeBridge } from "@takosjp/mobile-kit";
import {
  authorizeRunMutation,
  runMutationConfirmationMessage,
} from "../src/security.ts";
import type { MobileRun } from "../src/api.ts";

const standardRun = {
  id: "run_plan",
  workspaceId: "ws_a",
  capsuleId: "cap_a",
  type: "apply",
  status: "waiting_approval",
  createdBy: "acct_a",
  createdAt: "2026-07-20T00:00:00.000Z",
} satisfies MobileRun;

const restoreRun = {
  ...standardRun,
  id: "run_restore",
  type: "restore",
  backupId: "backup_7",
  restoreStateGeneration: 42,
} satisfies MobileRun;

test("restore approval uses a dedicated destructive confirmation", () => {
  expect(
    runMutationConfirmationMessage({
      kind: "approve",
      run: restoreRun,
      capsuleName: "Production",
    }),
  ).toContain("復元");
  expect(
    runMutationConfirmationMessage({
      kind: "approve",
      run: restoreRun,
      capsuleName: "Production",
    }),
  ).toContain("backup_7");
  expect(
    runMutationConfirmationMessage({
      kind: "approve",
      run: standardRun,
      capsuleName: "Production",
    }),
  ).not.toContain("backup_7");
});

test("a control mutation requires explicit confirmation before biometrics", async () => {
  const order: string[] = [];
  const bridge = biometricBridge(async () => {
    order.push("biometric");
    return true;
  });

  const allowed = await authorizeRunMutation({
    bridge,
    intent: { kind: "approve", run: restoreRun, capsuleName: "Production" },
    confirm: (input) => {
      order.push("confirm");
      expect(input.fallback).toBe(false);
      expect(input.message).toContain("復元");
      return true;
    },
  });

  expect(allowed).toBe(true);
  expect(order).toEqual(["confirm", "biometric"]);
});

test("a mutation fails closed without mobile biometrics", async () => {
  const bridge = biometricBridge(undefined);
  expect(
    await authorizeRunMutation({
      bridge,
      intent: { kind: "cancel", run: standardRun },
      confirm: () => true,
    }),
  ).toBe(false);
});

function biometricBridge(
  authenticateBiometric:
    | NativeBridge["authenticateBiometric"]
    | undefined,
): NativeBridge {
  return {
    capabilities: {
      launchPayload: false,
      launchPayloadEvents: false,
      externalBrowser: false,
      inAppBrowser: false,
      qrScanner: false,
      localNotifications: false,
      pushNotifications: false,
      biometricAuth: Boolean(authenticateBiometric),
      callIntent: false,
      clipboardText: false,
      secureStorage: true,
      persistentStorage: true,
    },
    getLaunchPayload: async () => undefined,
    openExternalUrl: async () => undefined,
    authenticateBiometric,
  };
}
