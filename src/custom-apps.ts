import type { MobilePersistentStore } from "@takosjp/mobile-kit";
import type { CustomAppEntry } from "./app-screens.ts";
import { normalizeUserAppUrl } from "./url-policy.ts";

/**
 * https URLs the person added to the home. They are stored only on this
 * device (the non-credential persistent store) and never sent to Takosumi.
 */

export const CUSTOM_APPS_STORAGE_KEY = "takosumi.mobile.custom-apps.v1";
export const MAX_CUSTOM_APPS = 100;
const TITLE_MAX_LENGTH = 64;
const ID_PATTERN = /^[A-Za-z0-9_-]{8,64}$/u;

type CustomAppsStore = Pick<MobilePersistentStore, "get" | "set">;

export async function readCustomApps(
  store: CustomAppsStore | undefined,
): Promise<readonly CustomAppEntry[]> {
  if (!store) return [];
  const raw = await store.get(CUSTOM_APPS_STORAGE_KEY);
  if (!raw) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  const seen = new Set<string>();
  const entries: CustomAppEntry[] = [];
  for (const item of parsed) {
    const entry = decodeEntry(item);
    if (!entry || seen.has(entry.id)) continue;
    seen.add(entry.id);
    entries.push(entry);
    if (entries.length >= MAX_CUSTOM_APPS) break;
  }
  return entries;
}

export type AddCustomAppResult =
  | {
      readonly ok: true;
      readonly entry: CustomAppEntry;
      readonly entries: readonly CustomAppEntry[];
      readonly existed: boolean;
    }
  | { readonly ok: false; readonly error: string };

export async function addCustomApp(input: {
  readonly store: CustomAppsStore | undefined;
  readonly url: string;
  readonly title?: string;
  readonly now?: () => Date;
  readonly createId?: () => string;
}): Promise<AddCustomAppResult> {
  if (!input.store) {
    return { ok: false, error: "この端末では追加したURLを保存できません。" };
  }
  const normalized = normalizeUserAppUrl(input.url);
  if (!normalized.ok) return normalized;
  const entries = await readCustomApps(input.store);
  const existing = entries.find((entry) => entry.url === normalized.url);
  if (existing) {
    return { ok: true, entry: existing, entries, existed: true };
  }
  if (entries.length >= MAX_CUSTOM_APPS) {
    return {
      ok: false,
      error: `追加できるURLは${MAX_CUSTOM_APPS}件までです。`,
    };
  }
  const id = (input.createId ?? randomId)();
  if (!ID_PATTERN.test(id)) throw new Error("custom app id is invalid");
  const entry: CustomAppEntry = {
    id,
    url: normalized.url,
    title: clampTitle(input.title) || normalized.host,
    addedAt: (input.now ?? (() => new Date()))().toISOString(),
  };
  const next = [...entries, entry];
  await input.store.set(CUSTOM_APPS_STORAGE_KEY, JSON.stringify(next));
  return { ok: true, entry, entries: next, existed: false };
}

export async function removeCustomApp(
  store: CustomAppsStore | undefined,
  id: string,
): Promise<readonly CustomAppEntry[]> {
  if (!store) return [];
  const entries = await readCustomApps(store);
  const next = entries.filter((entry) => entry.id !== id);
  if (next.length !== entries.length) {
    await store.set(CUSTOM_APPS_STORAGE_KEY, JSON.stringify(next));
  }
  return next;
}

function decodeEntry(value: unknown): CustomAppEntry | undefined {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return undefined;
  }
  const record = value as Record<string, unknown>;
  if (typeof record.id !== "string" || !ID_PATTERN.test(record.id)) {
    return undefined;
  }
  if (typeof record.url !== "string") return undefined;
  const normalized = normalizeUserAppUrl(record.url);
  if (!normalized.ok || normalized.url !== record.url) return undefined;
  const title =
    typeof record.title === "string" ? clampTitle(record.title) : "";
  const addedAt =
    typeof record.addedAt === "string" && !Number.isNaN(Date.parse(record.addedAt))
      ? record.addedAt
      : new Date(0).toISOString();
  return {
    id: record.id,
    url: normalized.url,
    title: title || normalized.host,
    addedAt,
  };
}

function clampTitle(value: string | undefined): string {
  const chars = [...(value ?? "").trim()];
  return chars.slice(0, TITLE_MAX_LENGTH).join("");
}

function randomId(): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}
