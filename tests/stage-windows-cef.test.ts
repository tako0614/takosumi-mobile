// scripts/stage-windows-cef.mjs on a synthetic CEF distribution: it only
// copies files, so it runs on any OS.
import { afterAll, describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

const SCRIPT = path.join(import.meta.dir, "..", "scripts", "stage-windows-cef.mjs");
const root = mkdtempSync(path.join(tmpdir(), "stage-windows-cef-"));
afterAll(() => rmSync(root, { recursive: true, force: true }));

/** A minimal PE header: "MZ", e_lfanew 0x80, "PE\0\0", the machine. */
function pe(machine = 0x8664, extra = ""): Buffer {
  const header = Buffer.alloc(0x100);
  header.write("MZ", 0, "latin1");
  header.writeUInt32LE(0x80, 0x3c);
  header.write("PE\0\0", 0x80, "latin1");
  header.writeUInt16LE(machine, 0x84);
  return Buffer.concat([header, Buffer.from(extra)]);
}

let cases = 0;
function distribution(extra: Record<string, Buffer | string> = {}) {
  const base = path.join(root, `case-${cases++}`);
  const cef = path.join(base, "cef_windows_x86_64");
  const bin = path.join(base, "debug");
  mkdirSync(path.join(cef, "locales"), { recursive: true });
  mkdirSync(path.join(cef, "include"), { recursive: true });
  mkdirSync(bin, { recursive: true });
  const files: Record<string, Buffer | string> = {
    "libcef.dll": pe(),
    "chrome_elf.dll": pe(),
    "icudtl.dat": "icu",
    "resources.pak": "resources",
    "chrome_100_percent.pak": "100",
    "chrome_200_percent.pak": "200",
    "v8_context_snapshot.bin": "snapshot",
    "vk_swiftshader.dll": pe(),
    "CREDITS.html": "<html>credits</html>",
    "libcef.lib": "import library",
    "archive.json": JSON.stringify({ name: "cef_binary_test_windows64_minimal.tar.bz2" }),
    "locales/en-US.pak": "en",
    "locales/ja.pak": "ja",
    ...extra,
  };
  for (const [name, content] of Object.entries(files)) writeFileSync(path.join(cef, name), content);
  writeFileSync(path.join(bin, "takosumi-cef-browser-process.exe"), pe());
  writeFileSync(path.join(bin, "takosumi-browser-guard.exe"), pe());
  return { base, cef, bin, out: path.join(base, "out") };
}

function stage(args: string[]) {
  return spawnSync(process.execPath, [SCRIPT, ...args], { encoding: "utf8" });
}

const sha256 = (file: string) => createHash("sha256").update(readFileSync(file)).digest("hex");

describe("stage-windows-cef", () => {
  test("copies exactly the runtime and both executables, with a manifest", () => {
    const { cef, bin, out } = distribution();
    const result = stage(["--cef", cef, "--bin", bin, "--out", out]);
    expect(result.status).toBe(0);
    const manifest = JSON.parse(readFileSync(path.join(out, "stage.json"), "utf8"));
    expect(manifest.kind).toBe("takosumi.windows-cef-stage@v1");
    expect(manifest.fixture).toBe(false);
    expect(manifest.inspectionSeam).toBe(false);
    expect(manifest.sandbox).toContain("unsandboxed");
    expect(manifest.cef.archive.name).toBe("cef_binary_test_windows64_minimal.tar.bz2");
    expect(manifest.files.map((file: { path: string }) => file.path).sort()).toEqual(
      [
        "takosumi-cef-browser-process.exe",
        "takosumi-browser-guard.exe",
        "libcef.dll",
        "chrome_elf.dll",
        "icudtl.dat",
        "resources.pak",
        "chrome_100_percent.pak",
        "chrome_200_percent.pak",
        "CREDITS.html",
        "v8_context_snapshot.bin",
        "vk_swiftshader.dll",
        "locales/en-US.pak",
        "locales/ja.pak",
      ].sort(),
    );
    for (const file of manifest.files) {
      expect(sha256(path.join(out, "cef", file.path))).toBe(file.sha256);
    }
    expect(manifest.notStaged).toContain("libcef.lib (import library (SDK build input))");
    expect(manifest.notStaged).toContain("include (SDK build input)");
  });

  test("refuses a fixture browser for a package and accepts it for a fixture stage", () => {
    const { cef, bin, out } = distribution();
    writeFileSync(path.join(bin, "takosumi-cef-browser-process.exe"), pe(0x8664, "TAKOSUMI_BROWSER_PROCESS_INSPECT"));
    const refused = stage(["--cef", cef, "--bin", bin, "--out", out]);
    expect(refused.status).toBe(2);
    expect(refused.stderr).toContain("inspection seam");
    const fixture = stage(["--cef", cef, "--bin", bin, "--out", out, "--fixture"]);
    expect(fixture.status).toBe(0);
    const manifest = JSON.parse(readFileSync(path.join(out, "stage.json"), "utf8"));
    expect(manifest.fixture).toBe(true);
    expect(manifest.inspectionSeam).toBe(true);
  });

  test("stops at an unknown DLL, a missing file, a wrong machine or an existing stage", () => {
    const unknown = distribution({ "brand-new.dll": pe() });
    const result = stage(["--cef", unknown.cef, "--bin", unknown.bin, "--out", unknown.out]);
    expect(result.status).toBe(2);
    expect(result.stderr).toContain("unknown DLLs");

    const missing = distribution();
    rmSync(path.join(missing.cef, "chrome_elf.dll"));
    expect(stage(["--cef", missing.cef, "--bin", missing.bin, "--out", missing.out]).status).toBe(2);

    const arm = distribution();
    writeFileSync(path.join(arm.bin, "takosumi-browser-guard.exe"), pe(0xaa64));
    const wrong = stage(["--cef", arm.cef, "--bin", arm.bin, "--out", arm.out]);
    expect(wrong.status).toBe(2);
    expect(wrong.stderr).toContain("x86-64");

    const twice = distribution();
    expect(stage(["--cef", twice.cef, "--bin", twice.bin, "--out", twice.out]).status).toBe(0);
    const again = stage(["--cef", twice.cef, "--bin", twice.bin, "--out", twice.out]);
    expect(again.status).toBe(2);
    expect(again.stderr).toContain("already exists");
  });
});
