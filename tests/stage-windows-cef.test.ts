// scripts/stage-windows-cef.mjs on a synthetic CEF distribution (it only
// copies files, so it runs on any OS), and its pure PE helpers
// (scripts/windows-cef-pe.mjs).
import { afterAll, describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

const SCRIPT = path.join(import.meta.dir, "..", "scripts", "stage-windows-cef.mjs");
const { cefLicenseFromHeader, peImports, systemDll, SYSTEM_DLLS } = await import(
  path.join(import.meta.dir, "..", "scripts", "windows-cef-pe.mjs")
);
const root = mkdtempSync(path.join(tmpdir(), "stage-windows-cef-"));
afterAll(() => rmSync(root, { recursive: true, force: true }));

/** A PE header without an optional header: "MZ", e_lfanew 0x80, "PE\0\0", the machine. */
function pe(machine = 0x8664): Buffer {
  const header = Buffer.alloc(0x100);
  header.write("MZ", 0, "latin1");
  header.writeUInt32LE(0x80, 0x3c);
  header.write("PE\0\0", 0x80, "latin1");
  header.writeUInt16LE(machine, 0x84);
  return header;
}

interface Layout {
  /** Offset of the import descriptors in the section. */
  descriptors?: number;
  /** Offset of the first name in the section. */
  names?: number;
  /** SizeOfRawData (the section's file data). */
  rawSize?: number;
  /** PointerToRawData as written; the data itself is always at 0x200. */
  pointer?: number;
  /** What the file holds past the section's file data. */
  fill?: number;
}

/**
 * An x86-64 PE32+ image with one section (RVA 0x1000, file data at 0x200)
 * holding an import directory that names `imports`, then `trailer`.
 */
function peWithImports(imports: string[], layout: Layout = {}, trailer = ""): Buffer {
  const { descriptors = 0, names = 0x100, rawSize = 0x1000, pointer = 0x200, fill = 0 } = layout;
  const file = Buffer.alloc(0x200 + 0x1000, fill);
  file.fill(0, 0, 0x200 + rawSize);
  file.write("MZ", 0, "latin1");
  file.writeUInt32LE(0x40, 0x3c);
  file.write("PE\0\0", 0x40, "latin1");
  file.writeUInt16LE(0x8664, 0x44); // machine
  file.writeUInt16LE(1, 0x46); // sections
  file.writeUInt16LE(240, 0x54); // optional header size
  const optional = 0x58;
  file.writeUInt16LE(0x20b, optional); // PE32+
  file.writeUInt32LE(16, optional + 108); // data directories
  file.writeUInt32LE(0x1000 + descriptors, optional + 112 + 8); // import directory RVA
  file.writeUInt32LE(20 * (imports.length + 1), optional + 112 + 12);
  const section = optional + 240;
  file.writeUInt32LE(0x1000, section + 8); // virtual size
  file.writeUInt32LE(0x1000, section + 12); // virtual address
  file.writeUInt32LE(rawSize, section + 16);
  file.writeUInt32LE(pointer, section + 20);
  let name = names;
  imports.forEach((dll, index) => {
    file.writeUInt32LE(0x1000 + name, 0x200 + descriptors + index * 20 + 12);
    file.write(`${dll}\0`, 0x200 + name, "latin1");
    name += dll.length + 1;
  });
  return Buffer.concat([file, Buffer.from(trailer)]);
}

const reader = (image: Buffer) => (offset: number, length: number) => image.subarray(offset, offset + length);

const CEF_BASE_H = [
  "// Copyright (c) 2014 Marshall A. Greenblatt. All rights reserved.",
  "//",
  "// Redistribution and use in source and binary forms, with or without",
  "// modification, are permitted provided that the following conditions are met.",
  "",
  "#ifndef CEF_INCLUDE_CEF_BASE_H_",
].join("\n");

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
    "chrome_elf.dll": peWithImports(["KERNEL32.dll", "ntdll.dll"]),
    "icudtl.dat": "icu",
    "resources.pak": "resources",
    "chrome_100_percent.pak": "100",
    "chrome_200_percent.pak": "200",
    "v8_context_snapshot.bin": "snapshot",
    "vk_swiftshader.dll": peWithImports(["KERNEL32.dll", "gdi32.dll"]),
    "CREDITS.html": "<html>credits</html>",
    "libcef.lib": "import library",
    "bootstrap.exe": peWithImports(["KERNEL32.dll", "ntdll.dll"], {}, "bootstrap"),
    "bootstrapc.exe": peWithImports(["KERNEL32.dll"], {}, "bootstrapc"),
    "README.txt": "readme",
    "archive.json": JSON.stringify({ name: "cef_binary_test_windows64_minimal.tar.bz2" }),
    "include/cef_base.h": CEF_BASE_H,
    "locales/en-US.pak": "en",
    "locales/ja.pak": "ja",
    ...extra,
  };
  for (const [name, content] of Object.entries(files)) writeFileSync(path.join(cef, name), content);
  // The sandboxed browser (the client DLL), the unsandboxed candidate and the guard.
  writeFileSync(path.join(bin, "takosumi_cef_browser.dll"), peWithImports(["KERNEL32.dll", "libcef.dll"], {}, "client"));
  writeFileSync(path.join(bin, "takosumi-cef-browser-process.exe"), peWithImports(["KERNEL32.dll", "libcef.dll"]));
  writeFileSync(path.join(bin, "takosumi-browser-guard.exe"), peWithImports(["KERNEL32.dll", "ntdll.dll"]));
  return { base, cef, bin, out: path.join(base, "out") };
}

function stage(args: string[], script = SCRIPT, runtime = process.execPath) {
  return spawnSync(runtime, [script, ...args], { encoding: "utf8" });
}

const sha256 = (file: string) => createHash("sha256").update(readFileSync(file)).digest("hex");
const product: string[] = [];

describe("windows-cef-pe", () => {
  test("reads a PE file's load-time imports", () => {
    const image = peWithImports(["KERNEL32.dll", "VCRUNTIME140.dll", "chrome_elf.dll"]);
    expect(peImports(reader(image))).toEqual(["KERNEL32.dll", "VCRUNTIME140.dll", "chrome_elf.dll"]);
    const broken = Buffer.from(image);
    broken.writeUInt32LE(0x9000, 0x58 + 112 + 8); // the import directory outside every section
    expect(() => peImports(reader(broken))).toThrow();
  });

  test("an image without an optional header is refused", () => {
    expect(() => peImports(reader(pe()))).toThrow("no optional header");
  });

  test("bytes past SizeOfRawData read as zero, as the loader maps them", () => {
    // The terminating descriptor lies past the section's file data, where
    // the file holds 0xAA bytes: the loader sees zeros, so one import.
    const image = peWithImports(["KERNEL32.dll"], { descriptors: 0x80, names: 0x10, rawSize: 0x80 + 20, fill: 0xaa });
    expect(peImports(reader(image))).toEqual(["KERNEL32.dll"]);
  });

  test("PointerToRawData is rounded down to 0x200, as the loader does", () => {
    const image = peWithImports(["KERNEL32.dll", "user32.dll"], { pointer: 0x2ff });
    expect(peImports(reader(image))).toEqual(["KERNEL32.dll", "user32.dll"]);
  });

  test("the system DLL list is exactly the CEF 154 runtime's imports", () => {
    expect([...SYSTEM_DLLS].sort()).toEqual([
      "advapi32.dll",
      "cfgmgr32.dll",
      "crypt32.dll",
      "dwrite.dll",
      "gdi32.dll",
      "kernel32.dll",
      "ntdll.dll",
      "ole32.dll",
      "oleaut32.dll",
      "rpcrt4.dll",
      "user32.dll",
      "version.dll",
      "winmm.dll",
      "ws2_32.dll",
    ]);
    expect(systemDll("KERNEL32.dll")).toBe(true);
    expect(systemDll("api-ms-win-core-synch-l1-2-0.dll")).toBe(true);
    for (const absent of ["VCRUNTIME140.dll", "MSVCP140.dll", "mf.dll", "mfplat.dll", "mfreadwrite.dll", "evr.dll"]) {
      expect(systemDll(absent)).toBe(false);
    }
  });

  test("CEF's license comes from the opening comment of its header", () => {
    const notice = cefLicenseFromHeader(CEF_BASE_H);
    expect(notice).toContain("Copyright (c) 2014 Marshall A. Greenblatt.");
    expect(notice).toContain("Redistribution and use in source and binary forms");
    expect(notice).not.toContain("//");
    expect(notice).not.toContain("#ifndef");
    expect(cefLicenseFromHeader("// Copyright someone else\n#pragma once")).toBeNull();
  });
});

describe("stage-windows-cef", () => {
  test("copies exactly the runtime and both executables, with a manifest", () => {
    const { cef, bin, out } = distribution();
    const result = stage(["--cef", cef, "--bin", bin, "--out", out, ...product]);
    expect(result.status).toBe(0);
    const manifest = JSON.parse(readFileSync(path.join(out, "stage.json"), "utf8"));
    expect(manifest.kind).toBe("takosumi.windows-cef-stage@v1");
    expect(manifest.fixture).toBe(false);
    expect(manifest.inspectionSeam).toBe(false);
    // The sandboxed browser: CEF's bootstrap under the browser's name and
    // the client DLL beside it, under the same name.
    expect(manifest.sandbox).toContain("bootstrap.exe");
    expect(manifest.unsandboxedCandidate).toBe(false);
    const staged = (name: string) => readFileSync(path.join(out, "cef", name));
    expect(staged("takosumi-cef-browser-process.exe")).toEqual(readFileSync(path.join(cef, "bootstrap.exe")));
    expect(staged("takosumi-cef-browser-process.dll")).toEqual(readFileSync(path.join(bin, "takosumi_cef_browser.dll")));
    expect(manifest.imports["takosumi-cef-browser-process.dll"]).toEqual(["KERNEL32.dll", "libcef.dll"]);
    expect(manifest.imports["takosumi-browser-guard.exe"]).toEqual(["KERNEL32.dll", "ntdll.dll"]);
    expect(manifest.cef.archive.name).toBe("cef_binary_test_windows64_minimal.tar.bz2");
    expect(manifest.cefLicense).toEqual({ notice: "CEF-LICENSE.txt", from: "include/cef_base.h" });
    expect(manifest.files.map((file: { path: string }) => file.path).sort()).toEqual(
      [
        "takosumi-cef-browser-process.exe",
        "takosumi-cef-browser-process.dll",
        "takosumi-browser-guard.exe",
        "libcef.dll",
        "chrome_elf.dll",
        "icudtl.dat",
        "resources.pak",
        "chrome_100_percent.pak",
        "chrome_200_percent.pak",
        "CREDITS.html",
        "CEF-LICENSE.txt",
        "v8_context_snapshot.bin",
        "vk_swiftshader.dll",
        "locales/en-US.pak",
        "locales/ja.pak",
      ].sort(),
    );
    for (const file of manifest.files) {
      expect(sha256(path.join(out, "cef", file.path))).toBe(file.sha256);
    }
    expect(readFileSync(path.join(out, "cef", "CEF-LICENSE.txt"), "utf8")).toContain("Marshall A. Greenblatt");
    expect(manifest.notStaged).toContain("libcef.lib (import library (SDK build input))");
    expect(manifest.notStaged).toContain("include (SDK build input)");
    expect(manifest.notStaged).toContain("README.txt (distribution documentation)");
    expect(manifest.notStaged).toContain(
      "bootstrapc.exe (CEF's console bootstrap; the browser is a Windows-subsystem program)",
    );
    expect(manifest.notStaged.join("\n")).not.toContain("bootstrap.exe (");
  });

  test("the unsandboxed candidate is staged for a fixture only", () => {
    const { cef, bin, out } = distribution();
    const refused = stage(["--cef", cef, "--bin", bin, "--out", out, "--unsandboxed-candidate"]);
    expect(refused.status).toBe(2);
    expect(refused.stderr).toContain("for a fixture only");
    const fixture = stage(["--cef", cef, "--bin", bin, "--out", out, "--fixture", "--unsandboxed-candidate"]);
    expect(fixture.status).toBe(0);
    const manifest = JSON.parse(readFileSync(path.join(out, "stage.json"), "utf8"));
    expect(manifest.unsandboxedCandidate).toBe(true);
    expect(manifest.sandbox).toContain("none");
    const paths = manifest.files.map((file: { path: string }) => file.path);
    expect(paths).not.toContain("takosumi-cef-browser-process.dll");
    expect(readFileSync(path.join(out, "cef", "takosumi-cef-browser-process.exe"))).toEqual(
      readFileSync(path.join(bin, "takosumi-cef-browser-process.exe")),
    );
    expect(manifest.notStaged).toContain(
      "bootstrap.exe (CEF's sandbox bootstrap, staged as the browser only for the sandboxed browser)",
    );
  });

  test("the sandboxed browser needs the client DLL and the bootstrap", () => {
    const noClient = distribution();
    rmSync(path.join(noClient.bin, "takosumi_cef_browser.dll"));
    const missing = stage(["--cef", noClient.cef, "--bin", noClient.bin, "--out", noClient.out]);
    expect(missing.status).toBe(2);
    expect(missing.stderr).toContain("takosumi_cef_browser.dll");
    const noBootstrap = distribution();
    rmSync(path.join(noBootstrap.cef, "bootstrap.exe"));
    const absent = stage(["--cef", noBootstrap.cef, "--bin", noBootstrap.bin, "--out", noBootstrap.out]);
    expect(absent.status).toBe(2);
    expect(absent.stderr).toContain("bootstrap.exe");
  });

  test("keeps CEF's license: its LICENSE.txt, CREDITS.html naming it, or nothing staged", () => {
    const licensed = distribution({ "LICENSE.txt": "Copyright Marshall A. Greenblatt; BSD" });
    expect(stage(["--cef", licensed.cef, "--bin", licensed.bin, "--out", licensed.out, ...product]).status).toBe(0);
    let manifest = JSON.parse(readFileSync(path.join(licensed.out, "stage.json"), "utf8"));
    expect(manifest.cefLicense.notice).toBe("LICENSE.txt");
    expect(manifest.files.map((file: { path: string }) => file.path)).toContain("LICENSE.txt");

    const credited = distribution({ "CREDITS.html": "<html>CEF: Copyright Marshall A. Greenblatt</html>" });
    expect(stage(["--cef", credited.cef, "--bin", credited.bin, "--out", credited.out, ...product]).status).toBe(0);
    manifest = JSON.parse(readFileSync(path.join(credited.out, "stage.json"), "utf8"));
    expect(manifest.cefLicense.notice).toBe("CREDITS.html");

    const unlicensed = distribution();
    rmSync(path.join(unlicensed.cef, "include", "cef_base.h"));
    const refused = stage(["--cef", unlicensed.cef, "--bin", unlicensed.bin, "--out", unlicensed.out, ...product]);
    expect(refused.status).toBe(2);
    expect(refused.stderr).toContain("no CEF license notice");
  });

  test("runs its checks when started through a symbolic link", () => {
    // Node keeps a linked main script's argv[1] unresolved while
    // import.meta.url is the real path: the script must not depend on
    // comparing them (it once exited 0 there without staging).
    const link = path.join(root, "linked-stage-windows-cef.mjs");
    symlinkSync(SCRIPT, link);
    const node = Bun.which("node");
    for (const runtime of [process.execPath, ...(node ? [node] : [])]) {
      const { cef, bin, out } = distribution({ "brand-new.dll": peWithImports(["KERNEL32.dll"]) });
      const result = stage(["--cef", cef, "--bin", bin, "--out", out, ...product], link, runtime);
      expect(result.status).toBe(2);
      expect(result.stderr).toContain("unknown entries");
      expect(stage([], link, runtime).status).toBe(2);
    }
  });

  test("refuses an import of the Visual C++ runtime or of anything not staged", () => {
    for (const dll of ["VCRUNTIME140.dll", "somewhere-else.dll", "mfplat.dll"]) {
      const { cef, bin, out } = distribution();
      writeFileSync(path.join(bin, "takosumi-browser-guard.exe"), peWithImports(["KERNEL32.dll", dll]));
      const result = stage(["--cef", cef, "--bin", bin, "--out", out, ...product]);
      expect(result.status).toBe(2);
      expect(result.stderr).toContain(`imports ${dll}`);
      expect(result.stderr).toContain("static C runtime");
    }
  });

  test("refuses a fixture browser for a package and accepts it for a fixture stage", () => {
    const { cef, bin, out } = distribution();
    writeFileSync(
      path.join(bin, "takosumi_cef_browser.dll"),
      peWithImports(["KERNEL32.dll", "libcef.dll"], {}, "TAKOSUMI_BROWSER_PROCESS_INSPECT"),
    );
    const refused = stage(["--cef", cef, "--bin", bin, "--out", out, ...product]);
    expect(refused.status).toBe(2);
    expect(refused.stderr).toContain("inspection seam");
    const fixture = stage(["--cef", cef, "--bin", bin, "--out", out, "--fixture"]);
    expect(fixture.status).toBe(0);
    const manifest = JSON.parse(readFileSync(path.join(out, "stage.json"), "utf8"));
    expect(manifest.fixture).toBe(true);
    expect(manifest.inspectionSeam).toBe(true);
  });

  test("stops at an unknown entry, a missing file, a wrong machine or an existing stage", () => {
    const extras: Record<string, Buffer | string>[] = [
      { "brand-new.dll": peWithImports(["KERNEL32.dll"]) },
      { "new_resources.pak": "x" },
      { "snapshot2.bin": "x" },
    ];
    for (const extra of extras) {
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

    const headerless = distribution({ "chrome_elf.dll": pe() });
    const unreadable = stage(["--cef", headerless.cef, "--bin", headerless.bin, "--out", headerless.out, ...product]);
    expect(unreadable.status).toBe(2);
    expect(unreadable.stderr).toContain("unreadable import table");

    const twice = distribution();
    expect(stage(["--cef", twice.cef, "--bin", twice.bin, "--out", twice.out, ...product]).status).toBe(0);
    const again = stage(["--cef", twice.cef, "--bin", twice.bin, "--out", twice.out, ...product]);
    expect(again.status).toBe(2);
    expect(again.stderr).toContain("already exists");
  });
});
