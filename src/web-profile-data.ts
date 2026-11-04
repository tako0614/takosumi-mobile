import type { AppScreenInvoke } from "./app-screen-native.ts";
import { connectionKey, webConnection, type WebConnection } from "./web-connections.ts";

/** A separate explicit action; removing a connection remains a preference edit. */
export function webProfileClearRequest(connection: WebConnection) {
  const checked = webConnection(connection.origin, connection.name, connection.profile);
  if (checked.profile === "shared") {
    throw new Error("以前の共有ログイン領域は、接続先ごとには消去できません。各サイトでログアウトしてください。");
  }
  return { profile: checked.profile, key: connectionKey(checked) };
}

export async function clearWebProfileData(connection: WebConnection, invoke: AppScreenInvoke): Promise<void> {
  const payload = webProfileClearRequest(connection);
  let result: unknown;
  try {
    result = await invoke<unknown>("plugin:app-screen|clear_profile", { payload });
  } catch (cause) {
    if (typeof cause === "string") throw new Error(cause);
    throw cause;
  }
  if (typeof result !== "object" || result === null ||
      (result as Record<string, unknown>).cleared !== true) {
    throw new Error("保存データの消去を確認できませんでした。");
  }
}
