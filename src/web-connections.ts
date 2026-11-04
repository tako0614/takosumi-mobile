/** Shell preferences only. Web sessions remain in the engine's cookie store. */
export interface WebConnection {
  readonly origin: string;
  readonly name: string;
}

export const CONNECTIONS_KEY = "takosumi.web-connections.v1";
export const WEB_CLIENT_KEY = "takosumi.web-client.selected.v1";

/** Remember an explicit mode choice so a native screen can reopen the shell. */
export function webClientSelected(
  search: string,
  storage?: Pick<Storage, "getItem" | "setItem">,
): boolean {
  const requested = new URLSearchParams(search).get("web-client");
  try {
    if (requested === "1" || requested === "0") storage?.setItem(WEB_CLIENT_KEY, requested);
    return requested === "1" || (requested !== "0" && storage?.getItem(WEB_CLIENT_KEY) === "1");
  } catch {
    return requested === "1";
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

export function webConnection(input: string, name = ""): WebConnection {
  const origin = connectionOrigin(input);
  const title = name.trim();
  if (title.length > 80 || /[\p{Cc}\p{Cf}]/u.test(title)) {
    throw new Error("接続先の名前は80文字以内で入力してください。");
  }
  return { origin, name: title || new URL(origin).host };
}

export function decodeConnections(raw: string | null): readonly WebConnection[] {
  if (raw === null) return [];
  try {
    const data: unknown = JSON.parse(raw);
    if (!Array.isArray(data) || data.length > 32) throw new Error();
    const result: WebConnection[] = [];
    for (const item of data) {
      if (!item || typeof item !== "object" ||
          typeof item.origin !== "string" || typeof item.name !== "string") throw new Error();
      const connection = webConnection(item.origin, item.name);
      if (!result.some((value) => value.origin === connection.origin)) result.push(connection);
    }
    return result;
  } catch {
    throw new Error("接続先の保存内容を読み込めませんでした。");
  }
}

export function webHomeRequest(connection: WebConnection) {
  const origin = connectionOrigin(connection.origin);
  return {
    key: `web-home:${origin}`,
    url: `${origin}/`,
    title: webConnection(origin, connection.name).name,
    profile: "shared",
    scope: `web-home:${origin}`,
    resume: true,
    webHome: true,
  } as const;
}
