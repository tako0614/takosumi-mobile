#!/usr/bin/env node
// Assemble the Windows App bundle's `cef/` resources (W6): the per-profile
// browser, its guard and the CEF runtime files they load from their own
// directory, copied from the locked CEF distribution and the built
// executables. src-tauri/tauri.windows-cef.conf.json maps the stage to the
// NSIS bundle's `cef/`, the same layout the Linux bundle uses
// (scripts/stage-linux-cef.mjs); an installed App resolves both executables
// there without environment variables.
//
//   node scripts/stage-windows-cef.mjs --cef <dir with libcef.dll> --bin <dir with the two .exe> --unsandboxed-candidate [--out <dir>]
//   node scripts/stage-windows-cef.mjs --fixture --cef <dir> --bin <dir> [--out <dir>]
//
// `--cef` is the flattened distribution cef-dll-sys unpacks into CEF_PATH
// (`cef_windows_x86_64`: Release and Resources together). `--out` defaults
// to src-tauri/target/windows-cef. The stage is `<out>/cef`; it must not
// exist yet (nothing is replaced or removed). Every file is copied (Windows
// bundles carry no links). A manifest with each staged file's size and
// SHA-256 is written beside it as `<out>/stage.json`, outside the bundled
// directory.
//
// A browser built with the cef-candidate feature `fixture-inspection` (it
// contains the TAKOSUMI_BROWSER_PROCESS_INSPECT seam) is refused unless
// `--fixture` is given. A fixture stage defaults to
// src-tauri/target/windows-cef-fixture (mapped by
// src-tauri/tauri.windows-cef-fixture.conf.json) and is never written to the
// package stage path. Every Windows browser is the unsandboxed candidate
// (cef-candidate feature `windows-unsandboxed-candidate`; a Windows build
// without it does not compile), so a package stage must say so explicitly
// (`--unsandboxed-candidate`); the manifest records it.
//
// Every staged executable and DLL must import at load time only Windows
// system DLLs (an explicit list, plus the api-ms-win-*/ext-ms-win-* API sets)
// or files staged beside it: a browser or guard built without the static C
// runtime would need VCRUNTIME140.dll, which a clean Windows lacks
// (native-tests/cef-candidate/.cargo/config.toml builds them with it). Any
// file in the distribution that is neither staged nor on the list of known
// ignorable entries stops the stage.
//
// Node or Bun. Only file copies: it runs on any OS, so it can be checked
// away from Windows (tests/stage-windows-cef.test.ts).

import { createHash } from "node:crypto";
import {
  closeSync,
  constants,
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  readSync,
  readdirSync,
  realpathSync,
  statSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const DEFAULT_OUT = path.join(REPO, "src-tauri", "target", "windows-cef");
const FIXTURE_OUT = path.join(REPO, "src-tauri", "target", "windows-cef-fixture");
/** Present only in a browser built with the fixture-inspection feature. */
const INSPECTION_SEAM = Buffer.from("TAKOSUMI_BROWSER_PROCESS_INSPECT");
/** IMAGE_FILE_MACHINE_AMD64 */
const AMD64 = 0x8664;

/** Built by native-tests/cef-candidate. */
const BINARIES = ["takosumi-cef-browser-process.exe", "takosumi-browser-guard.exe"];
/** CEF runtime files the browser needs beside it. */
const RUNTIME = {
  // libcef.dll imports chrome_elf.dll.
  required: [
    "libcef.dll",
    "chrome_elf.dll",
    "icudtl.dat",
    "resources.pak",
    "chrome_100_percent.pak",
    "chrome_200_percent.pak",
  ],
  // At least one V8 startup snapshot must be present.
  snapshots: ["v8_context_snapshot.bin", "snapshot_blob.bin"],
  // ANGLE, the shader compilers and software rendering (SwiftShader), when
  // this CEF ships them.
  optional: [
    "d3dcompiler_47.dll",
    "dxcompiler.dll",
    "dxil.dll",
    "libEGL.dll",
    "libGLESv2.dll",
    "vk_swiftshader.dll",
    "vk_swiftshader_icd.json",
    "vulkan-1.dll",
  ],
  // Chromium's third-party license notices travel with its binaries.
  notices: ["CREDITS.html"],
};
/** Never staged. The rest of the distribution is SDK build input. */
const EXCLUDED = new Map([
  ["libcef.lib", "import library (SDK build input)"],
  ["cef_sandbox.lib", "sandbox build input; the Windows browser runs unsandboxed"],
  ["bootstrap.exe", "CEF's sandbox bootstrap; the browser is its own executable"],
  ["bootstrapc.exe", "CEF's sandbox bootstrap; the browser is its own executable"],
  ["CMakeLists.txt", "SDK build input"],
  ["archive.json", "SDK download record (copied into stage.json)"],
  ["cmake", "SDK build input"],
  ["include", "SDK build input"],
  ["libcef_dll", "SDK build input"],
  ["README.txt", "distribution documentation"],
  ["LICENSE.txt", "distribution documentation"],
]);
/**
 * Windows system DLLs a staged file may import at load time (lower case),
 * besides the api-ms-win-* and ext-ms-win-* API sets. Not the Visual C++
 * runtime (vcruntime140*.dll, msvcp140*.dll and the like): a clean Windows
 * does not have it.
 */
const SYSTEM_DLLS = new Set([
  "advapi32.dll", "bcrypt.dll", "bcryptprimitives.dll", "cfgmgr32.dll", "comctl32.dll", "comdlg32.dll",
  "credui.dll", "crypt32.dll", "d2d1.dll", "d3d11.dll", "d3d12.dll", "d3d9.dll", "dbghelp.dll", "dcomp.dll",
  "dhcpcsvc.dll", "dnsapi.dll", "dwmapi.dll", "dwrite.dll", "dxgi.dll", "dxva2.dll", "esent.dll", "evr.dll",
  "fwpuclnt.dll", "gdi32.dll", "gdiplus.dll", "hid.dll", "iphlpapi.dll", "imm32.dll", "kernel32.dll",
  "kernelbase.dll", "mf.dll", "mfplat.dll", "mfreadwrite.dll", "mpr.dll", "mscms.dll", "msdmo.dll", "msimg32.dll",
  "msvcrt.dll", "ncrypt.dll", "netapi32.dll", "normaliz.dll", "ntdll.dll", "ole32.dll", "oleacc.dll",
  "oleaut32.dll", "opengl32.dll", "pdh.dll", "powrprof.dll", "propsys.dll", "psapi.dll", "rpcrt4.dll",
  "secur32.dll", "setupapi.dll", "shcore.dll", "shell32.dll", "shlwapi.dll", "sspicli.dll", "ucrtbase.dll",
  "uiautomationcore.dll", "urlmon.dll", "user32.dll", "userenv.dll", "usp10.dll", "uxtheme.dll", "version.dll",
  "wevtapi.dll", "winhttp.dll", "wininet.dll", "winmm.dll", "winspool.drv", "wintrust.dll", "winusb.dll",
  "wldap32.dll", "ws2_32.dll", "wsock32.dll", "wtsapi32.dll", "xinput1_4.dll",
]);

function usage(message) {
  if (message) console.error(`stage-windows-cef: ${message}`);
  console.error(
    "usage: stage-windows-cef.mjs --cef <dir with libcef.dll> --bin <dir with " +
      `${BINARIES.join(" and ")}> (--unsandboxed-candidate | --fixture) [--out <dir>]`,
  );
  process.exit(2);
}

function parse(argv) {
  const options = { out: undefined, fixture: false, unsandboxed: false };
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (flag === "--fixture") {
      options.fixture = true;
    } else if (flag === "--unsandboxed-candidate") {
      options.unsandboxed = true;
    } else if (flag === "--cef" || flag === "--bin" || flag === "--out") {
      const value = argv[index + 1];
      if (!value) usage(`${flag} needs a directory`);
      options[flag.slice(2)] = path.resolve(value);
      index += 1;
    } else {
      usage(`unknown argument ${flag}`);
    }
  }
  if (!options.cef || !options.bin) usage("--cef and --bin are required");
  if (!options.fixture && !options.unsandboxed) {
    usage(
      "every Windows browser is the unsandboxed candidate; a package stage needs --unsandboxed-candidate " +
        "to say so, so no unsandboxed installer is made by accident",
    );
  }
  options.out ??= options.fixture ? FIXTURE_OUT : DEFAULT_OUT;
  return options;
}

/** The PE machine of an executable or DLL, or null if it is not one. */
function peMachine(file) {
  const handle = openSync(file, "r");
  try {
    const header = Buffer.alloc(4096);
    const read = readSync(handle, header, 0, header.length, 0);
    if (read < 0x40 || header.toString("latin1", 0, 2) !== "MZ") return null;
    const offset = header.readUInt32LE(0x3c);
    if (offset + 6 > read || header.toString("latin1", offset, offset + 4) !== "PE\0\0") return null;
    return header.readUInt16LE(offset + 4);
  } finally {
    closeSync(handle);
  }
}

/**
 * The DLL names a PE file imports at load time (its import directory; the
 * delay-loaded ones are loaded on first use and not listed), read through
 * `read(offset, length)`, which returns a Buffer (shorter at the end of the
 * file). A file without an optional header or an import directory imports
 * nothing; a malformed one throws.
 */
export function peImports(read) {
  const dos = read(0, 64);
  if (dos.length < 64 || dos.toString("latin1", 0, 2) !== "MZ") throw new Error("no MZ header");
  const pe = dos.readUInt32LE(0x3c);
  const coff = read(pe, 24);
  if (coff.length < 24 || coff.toString("latin1", 0, 4) !== "PE\0\0") throw new Error("no PE header");
  const sectionCount = coff.readUInt16LE(6);
  const optionalSize = coff.readUInt16LE(20);
  if (optionalSize === 0) return [];
  const optional = read(pe + 24, optionalSize);
  if (optional.length < optionalSize) throw new Error("truncated optional header");
  const magic = optional.readUInt16LE(0);
  // NumberOfRvaAndSizes, then the data directories (PE32+ or PE32).
  const directories = magic === 0x20b ? 112 : magic === 0x10b ? 96 : -1;
  if (directories < 0 || optionalSize < directories) throw new Error(`unknown optional header 0x${magic.toString(16)}`);
  const count = optional.readUInt32LE(directories - 4);
  if (count < 2 || optionalSize < directories + 16) return [];
  const importRva = optional.readUInt32LE(directories + 8);
  if (importRva === 0) return [];
  const sections = read(pe + 24 + optionalSize, sectionCount * 40);
  if (sections.length < sectionCount * 40) throw new Error("truncated section table");
  const offsetOf = (rva) => {
    for (let index = 0; index < sectionCount; index += 1) {
      const base = index * 40;
      const virtualSize = sections.readUInt32LE(base + 8);
      const virtualAddress = sections.readUInt32LE(base + 12);
      const rawSize = sections.readUInt32LE(base + 16);
      const rawOffset = sections.readUInt32LE(base + 20);
      if (rva >= virtualAddress && rva < virtualAddress + Math.max(virtualSize, rawSize)) {
        return rawOffset + (rva - virtualAddress);
      }
    }
    throw new Error(`RVA 0x${rva.toString(16)} lies in no section`);
  };
  const names = [];
  for (let index = 0; index < 4096; index += 1) {
    const descriptor = read(offsetOf(importRva + index * 20), 20);
    if (descriptor.length < 20) throw new Error("truncated import directory");
    if (descriptor.every((byte) => byte === 0)) return names;
    const raw = read(offsetOf(descriptor.readUInt32LE(12)), 260);
    const end = raw.indexOf(0);
    if (end <= 0) throw new Error("unterminated import name");
    names.push(raw.toString("latin1", 0, end));
  }
  throw new Error("import directory without an end");
}

/** Whether a load-time import is a Windows system DLL. */
export function systemDll(name) {
  const lower = name.toLowerCase();
  return SYSTEM_DLLS.has(lower) || lower.startsWith("api-ms-win-") || lower.startsWith("ext-ms-win-");
}

function importsOf(file) {
  const handle = openSync(file, "r");
  try {
    return peImports((offset, length) => {
      const buffer = Buffer.alloc(length);
      return buffer.subarray(0, readSync(handle, buffer, 0, length, offset));
    });
  } finally {
    closeSync(handle);
  }
}

function sha256(file) {
  const hash = createHash("sha256");
  const handle = openSync(file, "r");
  try {
    const chunk = Buffer.alloc(1 << 20);
    for (;;) {
      const read = readSync(handle, chunk, 0, chunk.length, null);
      if (read === 0) break;
      hash.update(chunk.subarray(0, read));
    }
  } finally {
    closeSync(handle);
  }
  return hash.digest("hex");
}

/** The same location, also through links when both exist. */
function samePath(left, right) {
  if (path.resolve(left).toLowerCase() === path.resolve(right).toLowerCase()) return true;
  try {
    return realpathSync(left).toLowerCase() === realpathSync(right).toLowerCase();
  } catch {
    return false;
  }
}

function isLink(file) {
  try {
    return lstatSync(file).isSymbolicLink();
  } catch {
    return false;
  }
}

/** A regular file, never through a link (the stage copies what is there). */
function requireFile(file, what) {
  if (!existsSync(file) || isLink(file) || !statSync(file).isFile()) usage(`${what} not found or not a plain file: ${file}`);
  return file;
}

function requirePe(file) {
  const machine = peMachine(file);
  if (machine !== AMD64) {
    usage(`${file} is not an x86-64 Windows executable or DLL (machine ${machine === null ? "none" : `0x${machine.toString(16)}`})`);
  }
}

function main() {
  const options = parse(process.argv.slice(2));
  const cef = realpathSync(options.cef);
  const bin = realpathSync(options.bin);
  const stage = path.join(options.out, "cef");
  if (options.fixture && samePath(options.out, DEFAULT_OUT)) {
    usage(`a fixture stage is never written to the package stage path ${DEFAULT_OUT}`);
  }
  for (const existing of [stage, path.join(options.out, "stage.json")]) {
    if (existsSync(existing) || isLink(existing)) {
      usage(`${existing} already exists; remove it yourself or choose another --out`);
    }
  }

  // [stage-relative path, source]
  const plan = [];
  for (const name of BINARIES) {
    const source = requireFile(path.join(bin, name), `built ${name}`);
    requirePe(source);
    plan.push([name, source]);
  }
  const browser = plan[0][1];
  const inspectionSeam = readFileSync(browser).includes(INSPECTION_SEAM);
  if (inspectionSeam && !options.fixture) {
    usage(
      `${browser} contains the fixture inspection seam (built with --features fixture-inspection); ` +
        "a package stage refuses it. Rebuild without the feature, or pass --fixture for a fixture stage",
    );
  }
  if (options.fixture && !inspectionSeam) {
    console.log("note: this browser has no inspection seam; the fixture observer cannot attach to it");
  }
  for (const name of [...RUNTIME.required, ...RUNTIME.notices]) {
    plan.push([name, requireFile(path.join(cef, name), "CEF runtime file")]);
  }
  requirePe(path.join(cef, "libcef.dll"));
  const snapshots = RUNTIME.snapshots.filter((name) => existsSync(path.join(cef, name)));
  if (snapshots.length === 0) usage(`no V8 snapshot (${RUNTIME.snapshots.join(" or ")}) in ${cef}`);
  for (const name of snapshots) plan.push([name, requireFile(path.join(cef, name), "V8 snapshot")]);
  for (const name of RUNTIME.optional.filter((name) => existsSync(path.join(cef, name)))) {
    plan.push([name, requireFile(path.join(cef, name), "CEF runtime file")]);
  }
  const locales = path.join(cef, "locales");
  const paks = existsSync(locales) ? readdirSync(locales).filter((name) => name.endsWith(".pak")).sort() : [];
  if (!paks.includes("en-US.pak")) usage(`locales/en-US.pak is missing in ${cef}`);
  for (const name of paks) plan.push([path.join("locales", name), requireFile(path.join(locales, name), "locale")]);

  // Anything else in the distribution must be a known ignorable entry: an
  // unknown file may be a dependency or a resource this list does not know
  // yet, so it stops the stage.
  const staged = new Set(plan.map(([name]) => name.split(path.sep)[0]));
  const unknown = readdirSync(cef).filter((name) => !staged.has(name) && !EXCLUDED.has(name));
  if (unknown.length > 0) {
    usage(`unknown entries in ${cef}: ${unknown.join(", ")}; decide whether to stage or ignore them`);
  }
  const unknownLocales = existsSync(locales) ? readdirSync(locales).filter((name) => !name.endsWith(".pak")) : [];
  if (unknownLocales.length > 0) usage(`unknown entries in ${locales}: ${unknownLocales.join(", ")}`);
  const skipped = readdirSync(cef)
    .filter((name) => !staged.has(name))
    .map((name) => `${name} (${EXCLUDED.get(name)})`);

  // Load-time imports: only system DLLs or files staged beside them.
  const stagedNames = new Set(plan.map(([name]) => name.toLowerCase()));
  const imports = {};
  for (const [name, source] of plan) {
    if (!/\.(exe|dll)$/i.test(name)) continue;
    let names;
    try {
      names = importsOf(source);
    } catch (error) {
      usage(`${source}: unreadable import table (${error.message})`);
    }
    const missing = names.filter((dll) => !systemDll(dll) && !stagedNames.has(dll.toLowerCase()));
    if (missing.length > 0) {
      usage(
        `${name} imports ${missing.join(", ")}, neither a Windows system DLL nor staged; ` +
          "build the browser and guard with the static C runtime (native-tests/cef-candidate/.cargo/config.toml)",
      );
    }
    imports[name] = names;
  }

  mkdirSync(path.join(stage, "locales"), { recursive: true });
  const files = [];
  for (const [name, source] of plan) {
    const target = path.join(stage, name);
    copyFileSync(source, target, constants.COPYFILE_EXCL);
    files.push({ path: name.split(path.sep).join("/"), source, size: statSync(target).size, sha256: sha256(target) });
  }

  const archive = path.join(cef, "archive.json");
  const manifest = {
    kind: "takosumi.windows-cef-stage@v1",
    fixture: options.fixture,
    inspectionSeam,
    unsandboxedCandidate: true,
    sandbox: "none: the Windows browser is the unsandboxed candidate (windows-unsandboxed-candidate)",
    imports,
    cef: { directory: cef, archive: existsSync(archive) ? JSON.parse(readFileSync(archive, "utf8")) : null },
    bin,
    stage,
    notStaged: skipped,
    files,
  };
  // A new file only: never an existing manifest or a link planted in its place.
  writeFileSync(path.join(options.out, "stage.json"), `${JSON.stringify(manifest, null, 2)}\n`, { flag: "wx" });

  const total = files.reduce((sum, file) => sum + file.size, 0);
  console.log(`staged ${files.length} files (${(total / (1024 * 1024)).toFixed(1)} MiB) in ${stage}`);
  for (const line of skipped) console.log(`not staged: ${line}`);
  console.log(`manifest: ${path.join(options.out, "stage.json")}`);
  const [overlay, mapped] = options.fixture
    ? ["tauri.windows-cef-fixture.conf.json", FIXTURE_OUT]
    : ["tauri.windows-cef.conf.json", DEFAULT_OUT];
  if (!samePath(options.out, mapped)) {
    console.log(`note: src-tauri/${overlay} maps ${path.relative(REPO, mapped)}/cef/; pass an overlay that maps this stage`);
  }
}

// Run only as a program: tests import the pure helpers above.
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
