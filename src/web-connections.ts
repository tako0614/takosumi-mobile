/** Shell preferences only. Web sessions remain in the engine's cookie store. */
export interface WebConnection {
  readonly origin: string;
  readonly name: string;
  readonly profile: string;
}

export const CONNECTIONS_KEY = "takosumi.web-connections.v2";
export const LEGACY_CONNECTIONS_KEY = "takosumi.web-connections.v1";
export const WEB_CLIENT_KEY = "takosumi.web-client.selected.v1";

/** Open the Web home by default; retain an explicit legacy-mobile choice. */
export function webClientSelected(
  search: string,
  storage?: Pick<Storage, "getItem" | "setItem">,
): boolean {
  const requested = new URLSearchParams(search).get("web-client");
  try {
    if (requested === "1" || requested === "0") storage?.setItem(WEB_CLIENT_KEY, requested);
    return requested === "1" || (requested !== "0" && storage?.getItem(WEB_CLIENT_KEY) !== "0");
  } catch {
    return requested !== "0";
  }
}

export function connectionOrigin(input: string): string {
  const text = input.trim();
  if (!text || text.length > 2048 || /[\s\\]/u.test(text)) {
    throw new Error("Takosumiの接続先URLを入力してください。");
  }
  let url: URL;
  try {
    url = new URL(text);
  } catch {
    throw new Error("https:// から始まる接続先を入力してください。");
  }
  const host = url.hostname.toLowerCase().replace(/\.$/, "");
  if (
    url.protocol !== "https:" || !host ||
    host === "localhost" || host.endsWith(".localhost") ||
    url.username || url.password || url.pathname !== "/" ||
    url.search || url.hash || text.includes("?") || text.includes("#")
  ) {
    throw new Error("接続先には https://example.com のようにサイトのURLだけを指定してください。");
  }
  return url.origin;
}

export function isWebProfile(value: string): boolean {
  return value === "shared" || /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(value);
}

/** Use the same Web Crypto primitive as the mobile kit; no randomUUID capability is required. */
export function newWebProfile(random: Pick<Crypto, "getRandomValues"> = crypto): string {
  const bytes = new Uint8Array(16);
  random.getRandomValues(bytes);
  bytes[6] = (bytes[6]! & 0x0f) | 0x40;
  bytes[8] = (bytes[8]! & 0x3f) | 0x80;
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

export function webConnection(input: string, name = "", profile: string = newWebProfile()): WebConnection {
  const origin = connectionOrigin(input);
  const title = name.trim();
  if (title.length > 80 || /[\p{Cc}\p{Cf}]/u.test(title)) {
    throw new Error("接続先の名前は80文字以内で入力してください。");
  }
  if (!isWebProfile(profile)) throw new Error("接続先のログイン領域が正しくありません。");
  return { origin, name: title || new URL(origin).host.slice(0, 80), profile };
}

export function newWebConnection(current: readonly WebConnection[], input: string, name = ""): WebConnection {
  const connection = webConnection(input, name);
  const names = new Set(current.filter((item) => item.origin === connection.origin).map((item) => item.name));
  let title = connection.name;
  for (let number = 2; names.has(title); number++) {
    const suffix = `（${number}）`;
    title = connection.name.slice(0, 80 - suffix.length) + suffix;
  }
  return { ...connection, name: title };
}

export function connectionKey(connection: WebConnection): string {
  // Preserve the v1 shared screen identity and its restorable page.
  return connection.profile === "shared" ? `web-home:${connection.origin}` :
    `web-home:${connection.profile}:${connection.origin}`;
}

export function loadWebConnections(storage?: Pick<Storage, "getItem">): readonly WebConnection[] {
  if (!storage) return [];
  const current = storage.getItem(CONNECTIONS_KEY);
  return current !== null ? decodeConnections(current) :
    decodeConnections(storage.getItem(LEGACY_CONNECTIONS_KEY), true);
}

export function decodeConnections(raw: string | null, legacy = false): readonly WebConnection[] {
  if (raw === null) return [];
  try {
    const data: unknown = JSON.parse(raw);
    if (!Array.isArray(data) || data.length > 32) throw new Error();
    const result: WebConnection[] = [];
    for (const item of data) {
      if (!item || typeof item !== "object" ||
          typeof item.origin !== "string" || typeof item.name !== "string") throw new Error();
      const profile = legacy && item.profile === undefined ? "shared" : item.profile;
      if (typeof profile !== "string") throw new Error();
      const connection = webConnection(item.origin, item.name, profile);
      if (!result.some((value) => connectionKey(value) === connectionKey(connection))) result.push(connection);
    }
    return result;
  } catch {
    throw new Error("接続先の保存内容を読み込めませんでした。");
  }
}

export function webHomeRequest(connection: WebConnection) {
  const checked = webConnection(connection.origin, connection.name, connection.profile);
  const { origin, profile } = checked;
  return {
    key: connectionKey(checked),
    url: `${origin}/`,
    title: checked.name,
    profile,
    scope: connectionKey(checked),
    resume: true,
    webHome: true,
  } as const;
}
