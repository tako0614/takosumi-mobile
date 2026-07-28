import type {
  MobileKeyValueStore,
  MobileSession,
} from "@takosjp/mobile-kit";

export interface WorkspacePreference {
  readonly load: (session: MobileSession) => Promise<string | undefined>;
  readonly save: (session: MobileSession, workspaceId: string) => Promise<void>;
}

export function workspacePreferenceKey(
  session: Pick<MobileSession, "hostUrl" | "oidcClientId">,
): string {
  const host = new URL(session.hostUrl).origin;
  const client = session.oidcClientId?.trim() || "host-session";
  return `takosumi.mobile.workspace.${encodeURIComponent(host)}.${encodeURIComponent(client)}`;
}

/**
 * A Workspace id is non-secret navigation state. Keep it in the device store,
 * scoped to the operator host/client, and retain an in-memory fallback when
 * persistence is unavailable or temporarily fails.
 */
export function createWorkspacePreference(
  storage: MobileKeyValueStore | undefined,
): WorkspacePreference {
  const memory = new Map<string, string>();
  return {
    async load(session) {
      const key = workspacePreferenceKey(session);
      try {
        const stored = await storage?.get(key);
        if (stored?.trim()) {
          memory.set(key, stored);
          return stored;
        }
      } catch {
        // Navigation preference failure must not make the control API unusable.
      }
      return memory.get(key);
    },
    async save(session, workspaceId) {
      const value = workspaceId.trim();
      if (!value || value.length > 256) {
        throw new Error("Workspace id is invalid.");
      }
      const key = workspacePreferenceKey(session);
      memory.set(key, value);
      try {
        await storage?.set(key, value);
      } catch {
        // The in-memory preference remains valid for this process.
      }
    },
  };
}
