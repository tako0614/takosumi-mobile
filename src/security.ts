import {
  confirmMobileAction,
  type NativeBridge,
} from "@takosjp/mobile-kit";
import type { MobileRun } from "./api.ts";

export type RunMutationKind = "approve" | "cancel";

export interface RunMutationIntent {
  readonly kind: RunMutationKind;
  readonly run: MobileRun;
  readonly capsuleName?: string;
}

interface ConfirmationInput {
  readonly message: string;
  readonly fallback?: boolean;
}

export function runMutationConfirmationMessage(
  intent: RunMutationIntent,
): string {
  const target = intent.capsuleName?.trim() || "このサービス";
  if (intent.kind === "cancel") {
    return `${target} の実行を中止しますか？`;
  }
  if (intent.run.type !== "restore") {
    return `${target} の変更を承認しますか？`;
  }
  const backup = intent.run.backupId
    ? `バックアップ ${intent.run.backupId}`
    : "選択されたバックアップ";
  const generation =
    intent.run.restoreStateGeneration === undefined
      ? ""
      : `（状態世代 ${intent.run.restoreStateGeneration}）`;
  return `${target} を ${backup}${generation} から復元します。現在の状態は新しい世代で置き換えられます。復元を承認しますか？`;
}

/**
 * Mutating the control ledger requires two distinct user-presence checks:
 * an intent-specific confirmation and native biometric/device authentication.
 * Browser/desktop fallbacks deliberately cannot authorize mutations.
 */
export async function authorizeRunMutation(input: {
  readonly bridge: NativeBridge;
  readonly intent: RunMutationIntent;
  readonly confirm?: (input: ConfirmationInput) => boolean;
}): Promise<boolean> {
  const confirmed = (input.confirm ?? confirmMobileAction)({
    message: runMutationConfirmationMessage(input.intent),
    fallback: false,
  });
  if (!confirmed) return false;
  if (
    !input.bridge.capabilities.biometricAuth ||
    !input.bridge.authenticateBiometric
  ) {
    return false;
  }
  return await input.bridge.authenticateBiometric({
    title: "Takosumi",
    message:
      input.intent.kind === "approve"
        ? input.intent.run.type === "restore"
          ? "復元承認を認証します"
          : "変更承認を認証します"
        : "実行中止を認証します",
    allowDeviceCredential: true,
    confirmationRequired: true,
  });
}
