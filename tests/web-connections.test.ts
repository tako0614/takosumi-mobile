import { describe, expect, test } from "bun:test";
import { connectionOrigin, decodeConnections, webConnection, webHomeRequest, webClientSelected } from "../src/web-connections.ts";

describe("Web connection preferences", () => {
  test("keeps an explicit Web choice when the native screen cold-starts the shell, with legacy opt-out", () => {
    const saved = new Map<string, string>();
    const storage = { getItem: (key: string) => saved.get(key) ?? null,
      setItem: (key: string, value: string) => { saved.set(key, value); } };
    expect(webClientSelected("", storage)).toBe(false);
    expect(webClientSelected("?web-client=1", storage)).toBe(true);
    expect(webClientSelected("", storage)).toBe(true);
    expect(webClientSelected("?web-client=0", storage)).toBe(false);
    expect(webClientSelected("", storage)).toBe(false);
  });

  test("uses the site's Web root without constructing an API or OIDC credential", () => {
    const connection = webConnection(" https://MY.example:443/ ", " 自分の接続先 ");
    expect(connection).toEqual({ origin: "https://my.example", name: "自分の接続先" });
    expect(webHomeRequest(connection)).toEqual({
      key: "web-home:https://my.example", url: "https://my.example/",
      title: "自分の接続先", profile: "shared", scope: "web-home:https://my.example",
      resume: true, webHome: true,
    });
  });

  test("refuses pasted callback credentials, shell hosts, non-Web schemes and disguised hosts", () => {
    for (const input of [
      "https://my.example/sign-in/callback?code=secret", "https://my.example/?token=secret",
      "https://my.example/#access_token=secret", "https://my.example?", "https://my.example#",
      "https://user:password@my.example/", "https://user@my.example/", "https://my.example\\@evil.example",
      "http://my.example", "https://ipc.localhost", "https://tauri.localhost./", "https://localhost",
      "javascript:alert(1)", "file:///etc/passwd", "takosumi://oauth/callback", "",
    ]) expect(() => connectionOrigin(input)).toThrow();
  });

  test("restores only validated site preferences and deduplicates the same site", () => {
    expect(decodeConnections(JSON.stringify([
      { origin: "https://one.example", name: "One", accessToken: "never restore this" },
      { origin: "https://two.example", name: "Two" },
      { origin: "https://one.example/", name: "Duplicate" },
    ]))).toEqual([
      { origin: "https://one.example", name: "One" }, { origin: "https://two.example", name: "Two" },
    ]);
    expect(decodeConnections(null)).toEqual([]);
  });

  test("reports corrupt preferences instead of presenting an empty successful load", () => {
    for (const raw of ["", "{", "{}", '[{"origin":"https://evil.example/?token=secret","name":"Invalid"}]']) {
      expect(() => decodeConnections(raw)).toThrow("保存内容を読み込めません");
    }
  });

  test("never accepts control or bidi characters in a displayed connection name", () => {
    expect(() => webConnection("https://one.example", "one\u202eexample")).toThrow();
    expect(() => webConnection("https://one.example", "one\nexample")).toThrow();
  });
});
