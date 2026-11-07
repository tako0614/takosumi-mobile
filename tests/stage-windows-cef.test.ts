// scripts/stage-windows-cef.mjs on a synthetic CEF distribution: it only
// copies files, so it runs on any OS.
import { afterAll, describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

const SCRIPT = path.join(import.meta.dir, "..", "scripts", "stage-windows-cef.mjs");
const { peImports, systemDll } = await import(SCRIPT);
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

/**
 * An x86-64 PE32+ image with one section holding an import directory that
 * names `imports`, laid out as the Windows loader reads it.
 */
function peWithImports(imports: string[]): Buffer {
  const file = Buffer.alloc(0x200 + 0x1000);
  file.write("MZ", 0, "latin1");
  file.writeUInt32LE(0x40, 0x3c);
  file.write("PE\0\0", 0x40, "latin1");
  file.writeUInt16LE(0x8664, 0x44); // machine
  file.writeUInt16LE(1, 0x46); // sections
  file.writeUInt16LE(240, 0x54); // optional header size
  const optional = 0x58;
  file.writeUInt16LE(0x20b, optional); // PE32+
  file.writeUInt32LE(16, optional + 108); // data directories
  file.writeUInt32LE(0x1000, optional + 112 + 8); // import directory RVA
  file.writeUInt32LE(20 * (imports.length + 1), optional + 112 + 12);
  const section = optional + 240;
  file.writeUInt32LE(0x1000, section + 8); // virtual size
  file.writeUInt32LE(0x1000, section + 12); // virtual address
  file.writeUInt32LE(0x1000, section + 16); // raw size
  file.writeUInt32LE(0x200, section + 20); // raw offset
  let name = 0x100;
  imports.forEach((dll, index) => {
    file.writeUInt32LE(0x1000 + name, 0x200 + index * 20 + 12);
    file.write(`${dll}\0`, 0x200 + name, "latin1");
    name += dll.length + 1;
  });
  return file;
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
    "libcef.dll": peWithImports(["chrome_elf.dll", "KERNEL32.dll", "api-ms-win-core-synch-l1-2-0.dll"]),
    "chrome_elf.dll": pe(),
    "icudtl.dat": "icu",
    "resources.pak": "resources",
    "chrome_100_percent.pak": "100",
    "chrome_200_percent.pak": "200",
    "v8_context_snapshot.bin": "snapshot",
    "vk_swiftshader.dll": pe(),
    "CREDITS.html": "<html>credits</html>",
    "libcef.lib": "import library",
    "README.txt": "readme",
    "archive.json": JSON.stringify({ name: "cef_binary_test_windows64_minimal.tar.bz2" }),
    "locales/en-US.pak": "en",
    "locales/ja.pak": "ja",
    ...extra,
  };
  for (const [name, content] of Object.entries(files)) writeFileSync(path.join(cef, name), content);
  writeFileSync(path.join(bin, "takosumi-cef-browser-process.exe"), peWithImports(["KERNEL32.dll", "libcef.dll"]));
  writeFileSync(path.join(bin, "takosumi-browser-guard.exe"), peWithImports(["KERNEL32.dll", "ntdll.dll"]));
  return { base, cef, bin, out: path.join(base, "out") };
}

function stage(args: string[]) {
  return spawnSync(process.execPath, [SCRIPT, ...args], { encoding: "utf8" });
}

const sha256 = (file: string) => createHash("sha256").update(readFileSync(file)).digest("hex");

describe("stage-windows-cef", () => {
  test("reads a PE file's load-time imports", () => {
    const image = peWithImports(["KERNEL32.dll", "VCRUNTIME140.dll", "chrome_elf.dll"]);
    const read = (offset: number, length: number) => image.subarray(offset, offset + length);
    expect(peImports(read)).toEqual(["KERNEL32.dll", "VCRUNTIME140.dll", "chrome_elf.dll"]);
    expect(peImports((offset: number, length: number) => pe().subarray(offset, offset + length))).toEqual([]);
    const broken = Buffer.from(image);
    broken.writeUInt32LE(0x9000, 0x58 + 112 + 8); // the import directory outside every section
    expect(() => peImports((offset: number, length: number) => broken.subarray(offset, offset + length))).toThrow();
    expect(systemDll("KERNEL32.dll")).toBe(true);
    expect(systemDll("api-ms-win-core-synch-l1-2-0.dll")).toBe(true);
    expect(systemDll("VCRUNTIME140.dll")).toBe(false);
    expect(systemDll("MSVCP140.dll")).toBe(false);
  });

  test("copies exactly the runtime and both executables, with a manifest", () => {
    const { cef, bin, out } = distribution();
    const result = stage(["--cef", cef, "--bin", bin, "--out", out, "--unsandboxed-candidate"]);
    expect(result.status).toBe(0);
    const manifest = JSON.parse(readFileSync(path.join(out, "stage.json"), "utf8"));
    expect(manifest.kind).toBe("takosumi.windows-cef-stage@v1");
    expect(manifest.fixture).toBe(false);
    expect(manifest.inspectionSeam).toBe(false);
    expect(manifest.sandbox).toContain("unsandboxed");
    expect(manifest.unsandboxedCandidate).toBe(true);
    expect(manifest.imports["takosumi-browser-guard.exe"]).toEqual(["KERNEL32.dll", "ntdll.dll"]);
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
    expect(manifest.notStaged).toContain("README.txt (distribution documentation)");
  });

  test("a package stage must name the unsandboxed candidate", () => {
    const { cef, bin, out } = distribution();
    const refused = stage(["--cef", cef, "--bin", bin, "--out", out]);
    expect(refused.status).toBe(2);
    expect(refused.stderr).toContain("--unsandboxed-candidate");
  });

  test("refuses an import of the Visual C++ runtime or of anything not staged", () => {
    for (const dll of ["VCRUNTIME140.dll", "somewhere-else.dll"]) {
      const { cef, bin, out } = distribution();
      writeFileSync(path.join(bin, "takosumi-browser-guard.exe"), peWithImports(["KERNEL32.dll", dll]));
      const result = stage(["--cef", cef, "--bin", bin, "--out", out, "--unsandboxed-candidate"]);
      expect(result.status).toBe(2);
      expect(result.stderr).toContain(`imports ${dll}`);
      expect(result.stderr).toContain("static C runtime");
    }
  });

  test("refuses a fixture browser for a package and accepts it for a fixture stage", () => {
    const { cef, bin, out } = distribution();
    writeFileSync(path.join(bin, "takosumi-cef-browser-process.exe"), pe(0x8664, "TAKOSUMI_BROWSER_PROCESS_INSPECT"));
    const refused = stage(["--cef", cef, "--bin", bin, "--out", out, "--unsandboxed-candidate"]);
    expect(refused.status).toBe(2);
    expect(refused.stderr).toContain("inspection seam");
    const fixture = stage(["--cef", cef, "--bin", bin, "--out", out, "--fixture"]);
    expect(fixture.status).toBe(0);
    const manifest = JSON.parse(readFileSync(path.join(out, "stage.json"), "utf8"));
    expect(manifest.fixture).toBe(true);
    expect(manifest.inspectionSeam).toBe(true);
  });

  test("stops at an unknown entry, a missing file, a wrong machine or an existing stage", () => {
    const product = ["--unsandboxed-candidate"];
    for (const extra of [{ "brand-new.dll": pe() }, { "new_resources.pak": "x" }, { "snapshot2.bin": "x" }]) {
      const unknown = distribution(extra);
      const result = stage(["--cef", unknown.cef, "--bin", unknown.bin, "--out", unknown.out, ...product]);
      expect(result.status).toBe(2);
      expect(result.stderr).toContain("unknown entries");
    }

    const missing = distribution();
    rmSync(path.join(missing.cef, "chrome_elf.dll"));
    expect(stage(["--cef", missing.cef, "--bin", missing.bin, "--out", missing.out, ...product]).status).toBe(2);

    const arm = distribution();
    writeFileSync(path.join(arm.bin, "takosumi-browser-guard.exe"), pe(0xaa64));
    const wrong = stage(["--cef", arm.cef, "--bin", arm.bin, "--out", arm.out, ...product]);
    expect(wrong.status).toBe(2);
    expect(wrong.stderr).toContain("x86-64");

    const twice = distribution();
    expect(stage(["--cef", twice.cef, "--bin", twice.bin, "--out", twice.out, ...product]).status).toBe(0);
    const again = stage(["--cef", twice.cef, "--bin", twice.bin, "--out", twice.out, ...product]);
    expect(again.status).toBe(2);
    expect(again.stderr).toContain("already exists");
  });
});
