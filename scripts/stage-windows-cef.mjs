#!/usr/bin/env node
// Assemble the Windows App bundle's `cef/` resources (W6): the per-profile
// browser, its guard and the CEF runtime files they load from their own
// directory, copied from the locked CEF distribution and the built
// executables. src-tauri/tauri.windows-cef.conf.json maps the stage to the
// NSIS bundle's `cef/`, the same layout the Linux bundle uses
// (scripts/stage-linux-cef.mjs); an installed App resolves both executables
// there without environment variables.
//
//   node scripts/stage-windows-cef.mjs --cef <dir with libcef.dll> --bin <dir with the two .exe> [--out <dir>] [--fixture]
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
// without it does not compile), and the manifest says so.
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
]);

function usage(message) {
  if (message) console.error(`stage-windows-cef: ${message}`);
  console.error(
    "usage: stage-windows-cef.mjs --cef <dir with libcef.dll> --bin <dir with " +
      `${BINARIES.join(" and ")}> [--out <dir>] [--fixture]`,
  );
  process.exit(2);
}

function parse(argv) {
  const options = { out: undefined, fixture: false };
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (flag === "--fixture") {
      options.fixture = true;
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

  // Anything else in the distribution is named; an unknown DLL may be a
  // dependency this list does not know yet, so it stops the stage.
  const staged = new Set(plan.map(([name]) => name.split(path.sep)[0]));
  const unknown = readdirSync(cef).filter((name) => !staged.has(name) && !EXCLUDED.has(name));
  const unknownDlls = unknown.filter((name) => name.toLowerCase().endsWith(".dll"));
  if (unknownDlls.length > 0) {
    usage(`unknown DLLs in ${cef}: ${unknownDlls.join(", ")}; decide whether the browser needs them before staging`);
  }
  const skipped = readdirSync(cef)
    .filter((name) => !staged.has(name))
    .map((name) => `${name}${EXCLUDED.has(name) ? ` (${EXCLUDED.get(name)})` : " (unknown; not staged)"}`);

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
    sandbox: "none: the Windows browser is the unsandboxed candidate (windows-unsandboxed-candidate)",
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

main();
