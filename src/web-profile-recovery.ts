import type { AppScreenInvoke } from "./app-screen-native.ts";
import { connectionKey, webHomeRequest, type WebConnection } from "./web-connections.ts";

export class BrowserRecoveryRequired extends Error {
  constructor(
    readonly profile: string,
    readonly key: string,
    readonly generation: string,
  ) {
    super("ブラウザが正常に終了しませんでした。保存データは保持しています。");
  }
}

function matches(connection: WebConnection, value: Record<string, unknown>): boolean {
  return connection.profile !== "shared" && value.profile === connection.profile &&
    value.key === connectionKey(connection) && typeof value.generation === "string" &&
    /^[1-9][0-9]{0,19}$/.test(value.generation);
}

function nativeFailure(connection: WebConnection, cause: unknown): Error {
  if (typeof cause === "object" && cause !== null) {
    const value = cause as Record<string, unknown>;
    if (value.kind === "browserProfileRecoveryRequired" && matches(connection, value)) {
      return new BrowserRecoveryRequired(connection.profile, connectionKey(connection), value.generation as string);
    }
  }
  return cause instanceof Error ? cause : new Error(typeof cause === "string" ? cause : "接続先を開けませんでした。");
}

export async function openWebConnection(connection: WebConnection, invoke: AppScreenInvoke): Promise<void> {
  const payload = webHomeRequest(connection);
  try {
    await invoke("plugin:app-screen|open", { payload });
  } catch (cause) {
    throw nativeFailure(connection, cause);
  }
}

/** Only an explicit UI action may send the exact native generation ticket. */
export async function recoverWebProfile(
  connection: WebConnection, ticket: BrowserRecoveryRequired, invoke: AppScreenInvoke,
): Promise<void> {
  const open = webHomeRequest(connection);
  if (!(ticket instanceof BrowserRecoveryRequired) || !matches(connection, { ...ticket })) {
    throw new Error("復帰する接続先が変わっています。もう一度開いてください。");
  }
  let result: unknown;
  try {
    result = await invoke("plugin:app-screen|recover_profile", { payload: { open, generation: ticket.generation } });
  } catch (cause) {
    throw nativeFailure(connection, cause);
  }
  if (typeof result !== "object" || result === null || (result as Record<string, unknown>).reopened !== true) {
    throw new Error("ブラウザの復帰を確認できませんでした。");
  }
}
