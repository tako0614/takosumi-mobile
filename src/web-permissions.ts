import { listen } from "@tauri-apps/api/event";
import type { AppScreenInvoke } from "./app-screen-native.ts";

/** One site permission prompt a connection's browser is holding for the App. */
export interface PendingPermission {
  readonly profile: string;
  readonly promptId: number;
  readonly origin: string;
  readonly kind: "prompt" | "media";
  readonly permissions: number;
}

export type PermissionAnswer = "allow" | "deny";

export interface WebPermissionPrompts {
  /** Every prompt currently held; empty when the surface is unavailable. */
  readonly list: () => Promise<readonly PendingPermission[]>;
  /** Answer one held prompt; resolves the prompts still held after it. */
  readonly answer: (
    prompt: PendingPermission,
    decision: PermissionAnswer,
  ) => Promise<readonly PendingPermission[]>;
  /** Subscribe to held-prompt changes; resolves an unsubscribe. */
  readonly watch: (
    handler: (list: readonly PendingPermission[]) => void,
  ) => Promise<() => void>;
}

const PERMISSION_PROMPTS_EVENT = "app-screen:permission-prompts";

export function createWebPermissionPrompts(
  invoke: AppScreenInvoke,
): WebPermissionPrompts {
  return {
    async list() {
      const value = await invoke<unknown>("plugin:app-screen|pending_permissions").catch(
        () => [],
      );
      return decodePendingPermissions(value) ?? [];
    },
    async answer(prompt, decision) {
      const value = await invoke<unknown>("plugin:app-screen|resolve_permission", {
        payload: {
          profile: prompt.profile,
          promptId: prompt.promptId,
          decision,
        },
      });
      return decodePendingPermissions(value) ?? [];
    },
    async watch(handler) {
      const unlisten = await listen<unknown>(PERMISSION_PROMPTS_EVENT, (event) => {
        const list = decodePendingPermissions(event.payload);
        if (list) handler(list);
      });
      return () => {
        void unlisten();
      };
    },
  };
}

export function decodePendingPermissions(
  value: unknown,
): PendingPermission[] | undefined {
  if (!Array.isArray(value)) return undefined;
  const decoded: PendingPermission[] = [];
  for (const entry of value) {
    if (typeof entry !== "object" || entry === null) continue;
    const record = entry as Record<string, unknown>;
    const kind = record.kind === "media" ? "media" : "prompt";
    if (
      typeof record.profile !== "string" || !record.profile ||
      typeof record.promptId !== "number" ||
      !Number.isSafeInteger(record.promptId) || record.promptId <= 0 ||
      typeof record.origin !== "string" || !record.origin.startsWith("https://") ||
      typeof record.permissions !== "number" ||
      !Number.isSafeInteger(record.permissions) || record.permissions < 0
    ) continue;
    decoded.push({
      profile: record.profile,
      promptId: record.promptId,
      origin: record.origin,
      kind,
      permissions: record.permissions,
    });
  }
  return decoded;
}

// Chromium's cef_permission_type_t bits, as the browser-process pipe reports
// them. Unknown bits read as a generic label so a new permission never goes
// unanswered for want of a name.
const PERMISSION_LABELS: readonly (readonly [number, string])[] = [
  [1 << 2, "カメラ"],
  [1 << 12, "マイク"],
  [1 << 15, "通知"],
  [1 << 8, "位置情報"],
  [1 << 4, "クリップボード"],
  [1 << 7, "端末のフォント"],
  [1 << 24, "ファイルへのアクセス"],
  [1 << 23, "ウィンドウの管理"],
  [1 << 14, "連続したダウンロード"],
  [1 << 19, "リンクを開くアプリの登録"],
  [1 << 5, "別サイトをまたぐ保存アクセス"],
  [1 << 20, "埋め込み先への保存アクセス"],
  [1 << 13, "MIDIデバイス"],
  [1 << 18, "保護されたコンテンツの識別"],
  [1 << 6, "追加の保存容量"],
  [1 << 16, "キーボードの独占"],
  [1 << 17, "ポインタの独占"],
  [1 << 22, "アプリのインストール"],
  [1 << 11, "操作していない時間の検出"],
  [1 << 21, "VRセッション"],
  [1 << 0, "ARセッション"],
  [1 << 1, "カメラのパン・ズーム"],
  [1 << 3, "共有した画面の操作"],
  [1 << 9, "手の動きの検出"],
  [1 << 10, "ログイン基盤との連携"],
  [1 << 25, "ローカルネットワーク"],
  [1 << 26, "ローカルネットワーク"],
  [1 << 27, "この端末内のサービス"],
];

export function permissionLabels(permissions: number): readonly string[] {
  const labels: string[] = [];
  let unknown = permissions;
  for (const [bit, label] of PERMISSION_LABELS) {
    if (permissions & bit) {
      labels.push(label);
      unknown &= ~bit;
    }
  }
  if (unknown || labels.length === 0) labels.push("このサイトの機能");
  return labels;
}
