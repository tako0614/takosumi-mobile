import { describe, expect, test } from "bun:test";
import { CONNECTIONS_KEY, LEGACY_CONNECTIONS_KEY, connectionKey, connectionOrigin, decodeConnections,
  isWebProfile, loadWebConnections, newWebConnection, webConnection, webHomeRequest, webClientSelected } from "../src/web-connections.ts";
import profileCases from "../src-tauri/plugins/app-screen/profile-cases.json";

const firstProfile = "11111111-1111-4111-8111-111111111111";
const secondProfile = "22222222-2222-4222-8222-222222222222";

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
    const connection = webConnection(" https://MY.example:443/ ", " 自分の接続先 ", "shared");
    expect(connection).toEqual({ origin: "https://my.example", name: "自分の接続先", profile: "shared" });
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
    ]), true)).toEqual([
      { origin: "https://one.example", name: "One", profile: "shared" },
      { origin: "https://two.example", name: "Two", profile: "shared" },
    ]);
    expect(decodeConnections(null)).toEqual([]);
  });

  test("two logins at the same site retain separate identities through save, restore and native open", () => {
    const first = webConnection("https://sumi.example", "Work", firstProfile);
    const second = webConnection("https://sumi.example", "Personal", secondProfile);
    expect(connectionKey(first)).not.toBe(connectionKey(second));
    expect(decodeConnections(JSON.stringify([first, second]))).toEqual([first, second]);
    expect(webHomeRequest(first).profile).toBe(firstProfile);
    expect(webHomeRequest(second).profile).toBe(secondProfile);
    expect(webHomeRequest(first).scope).not.toBe(webHomeRequest(second).scope);
    const generated = webConnection("https://other.example");
    expect(isWebProfile(generated.profile)).toBe(true);
    expect(generated.profile).not.toBe("shared");
  });

  test("duplicate unnamed or identically named logins have stable distinguishable labels", () => {
    const first = newWebConnection([], "https://sumi.example");
    const second = newWebConnection([first], "https://sumi.example");
    const third = newWebConnection([first, second], "https://sumi.example");
    expect([first.name, second.name, third.name]).toEqual(["sumi.example", "sumi.example（2）", "sumi.example（3）"]);
    expect(newWebConnection([first], "https://sumi.example", "Work").name).toBe("Work");
    const named = newWebConnection([], "https://sumi.example", "x".repeat(80));
    const duplicate = newWebConnection([named], "https://sumi.example", "x".repeat(80));
    expect(duplicate.name.length).toBeLessThanOrEqual(80);
    expect(duplicate.name).not.toBe(named.name);
  });

  test("reads the old shared entries without modifying credentials or the v1 preference", () => {
    const old = JSON.stringify([{ origin: "https://one.example", name: "Old" }]);
    const data = new Map([[LEGACY_CONNECTIONS_KEY, old], ["native.credentials", "untouched"]]);
    const storage = { getItem: (key: string) => data.get(key) ?? null };
    expect(loadWebConnections(storage)[0]?.profile).toBe("shared");
    expect(data.get(LEGACY_CONNECTIONS_KEY)).toBe(old);
    expect(data.get("native.credentials")).toBe("untouched");
    data.set(CONNECTIONS_KEY, "[]");
    expect(loadWebConnections(storage)).toEqual([]);
    data.set(CONNECTIONS_KEY, "broken");
    expect(() => loadWebConnections(storage)).toThrow();
  });

  test("shares the canonical profile-name rules with native validators", () => {
    for (const item of profileCases.cases) expect(isWebProfile(item.profile)).toBe(item.valid);
    expect(() => decodeConnections(JSON.stringify([{ origin: "https://one.example", name: "Missing profile" }]))).toThrow();
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
