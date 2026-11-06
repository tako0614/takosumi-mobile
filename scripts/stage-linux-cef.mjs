#!/usr/bin/env node
// Assemble the Linux App bundle's `cef/` resources for the `desktop-cef`
// build: the per-profile browser, its guard and the CEF runtime files they
// load from their own directory. ELF files are stripped into the stage (the
// SDK's libcef.so carries about 1.2 GB of debug data); unchanged data files
// are symbolic links to their sources unless `--copy` is given. Tauri follows
// the links when it copies resources into the build and the bundles.
//
//   bun scripts/stage-linux-cef.mjs --cef <dir with libcef.so> --bin <dir with the two binaries> [--out <dir>] [--copy] [--fixture]
//
// `--out` defaults to src-tauri/target/linux-cef, the path that
// src-tauri/tauri.linux-cef.conf.json maps to the bundle's `cef/`. The stage
// is `<out>/cef`; it must not exist yet (nothing is replaced or removed). A
// manifest with each staged file's size and SHA-256 is written beside it as
// `<out>/stage.json`, outside the bundled directory.
//
// A browser built with the cef-candidate feature `fixture-inspection` (it
// contains the TAKOSUMI_BROWSER_PROCESS_INSPECT seam) is refused unless
// `--fixture` is given. A fixture stage defaults to
// src-tauri/target/linux-cef-fixture (mapped by
// src-tauri/tauri.linux-cef-fixture.conf.json) and is never written to the
// package stage path, so a package cannot silently pick up a fixture browser.
//
// Node or Bun, Linux x86-64, and binutils `strip` are required.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  openSync,
  closeSync,
  constants,
  readFileSync,
  readSync,
  readdirSync,
  realpathSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const DEFAULT_OUT = path.join(REPO, "src-tauri", "target", "linux-cef");
const FIXTURE_OUT = path.join(REPO, "src-tauri", "target", "linux-cef-fixture");
/** Present only in a browser built with the fixture-inspection feature. */
const INSPECTION_SEAM = Buffer.from("TAKOSUMI_BROWSER_PROCESS_INSPECT");

/** Built by native-tests/cef-candidate; both are stripped into the stage. */
const BINARIES = ["takosumi-cef-browser-process", "takosumi-browser-guard"];
/** CEF runtime files the browser needs beside libcef.so. */
const RUNTIME = {
  strip: ["libcef.so"],
  data: [
    "icudtl.dat",
    "resources.pak",
    "chrome_100_percent.pak",
    "chrome_200_percent.pak",
  ],
  // At least one V8 startup snapshot must be present.
  snapshots: ["v8_context_snapshot.bin", "snapshot_blob.bin"],
  // Software rendering (SwiftShader) and ANGLE, when this CEF ships them.
  optionalStrip: ["libvk_swiftshader.so", "libvulkan.so.1", "libEGL.so", "libGLESv2.so"],
  optionalData: ["vk_swiftshader_icd.json"],
  // Chromium's third-party license notices travel with its binaries.
  notices: ["CREDITS.html"],
};
/**
 * Never staged. `chrome-sandbox` works only as a setuid-root helper, which
 * neither the deb nor the AppImage installs; the browser relies on Chromium's
 * user-namespace sandbox instead. The rest is SDK build input.
 */
const EXCLUDED = new Map([
  ["chrome-sandbox", "setuid-root helper; the user-namespace sandbox is used"],
  ["CMakeLists.txt", "SDK build input"],
  ["archive.json", "SDK download record (copied into stage.json)"],
  ["cmake", "SDK build input"],
  ["include", "SDK build input"],
  ["libcef_dll", "SDK build input"],
]);

function usage(message) {
  if (message) console.error(`stage-linux-cef: ${message}`);
  console.error(
    "usage: stage-linux-cef.mjs --cef <dir with libcef.so> --bin <dir with " +
      `${BINARIES.join(" and ")}> [--out <dir>] [--copy] [--fixture]`,
  );
  process.exit(2);
}

function parse(argv) {
  const options = { out: undefined, copy: false, fixture: false };
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (flag === "--copy") {
      options.copy = true;
    } else if (flag === "--fixture") {
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

function isElf(file) {
  const handle = openSync(file, "r");
  try {
    const magic = Buffer.alloc(4);
    return readSync(handle, magic, 0, 4, 0) === 4 && magic.equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46]));
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
  if (path.resolve(left) === path.resolve(right)) return true;
  try {
    return realpathSync(left) === realpathSync(right);
  } catch {
    return false;
  }
}

function requireFile(file, what) {
  if (!existsSync(file) || !statSync(file).isFile()) usage(`${what} not found: ${file}`);
  return realpathSync(file);
}

function main() {
  if (process.platform !== "linux" || process.arch !== "x64") {
    usage("the Linux CEF stage is built on Linux x86-64 only");
  }
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
  if (spawnSync("strip", ["--version"], { stdio: "ignore" }).status !== 0) {
    usage("binutils strip is required");
  }

  // [stage-relative path, source, "strip" | "data"]
  const plan = [];
  for (const name of BINARIES) {
    const source = requireFile(path.join(bin, name), `built ${name}`);
    if (!isElf(source)) usage(`${source} is not an ELF executable`);
    plan.push([name, source, "strip"]);
  }
  const browser = plan.find(([name]) => name === "takosumi-cef-browser-process")[1];
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
  for (const name of RUNTIME.strip) plan.push([name, requireFile(path.join(cef, name), "CEF runtime file"), "strip"]);
  for (const name of [...RUNTIME.data, ...RUNTIME.notices]) {
    plan.push([name, requireFile(path.join(cef, name), "CEF runtime file"), "data"]);
  }
  const snapshots = RUNTIME.snapshots.filter((name) => existsSync(path.join(cef, name)));
  if (snapshots.length === 0) usage(`no V8 snapshot (${RUNTIME.snapshots.join(" or ")}) in ${cef}`);
  for (const name of snapshots) plan.push([name, realpathSync(path.join(cef, name)), "data"]);
  for (const name of RUNTIME.optionalStrip.filter((name) => existsSync(path.join(cef, name)))) {
    plan.push([name, realpathSync(path.join(cef, name)), "strip"]);
  }
  for (const name of RUNTIME.optionalData.filter((name) => existsSync(path.join(cef, name)))) {
    plan.push([name, realpathSync(path.join(cef, name)), "data"]);
  }
  const locales = path.join(cef, "locales");
  const paks = existsSync(locales) ? readdirSync(locales).filter((name) => name.endsWith(".pak")).sort() : [];
  if (!paks.includes("en-US.pak")) usage(`locales/en-US.pak is missing in ${cef}`);
  for (const name of paks) plan.push([path.join("locales", name), realpathSync(path.join(locales, name)), "data"]);

  const staged = new Set(plan.map(([name]) => name.split(path.sep)[0]));
  const skipped = readdirSync(cef)
    .filter((name) => !staged.has(name))
    .map((name) => `${name}${EXCLUDED.has(name) ? ` (${EXCLUDED.get(name)})` : " (unknown; not staged)"}`);

  mkdirSync(path.join(stage, "locales"), { recursive: true, mode: 0o755 });
  const files = [];
  for (const [name, source, how] of plan) {
    const target = path.join(stage, name);
    let method = how;
    if (how === "strip") {
      const result = spawnSync("strip", ["--strip-unneeded", "-o", target, source], { encoding: "utf8" });
      if (result.status !== 0) {
        console.error(result.stderr);
        usage(`strip failed for ${source}`);
      }
      chmodSync(target, 0o755);
    } else if (options.copy) {
      copyFileSync(source, target, constants.COPYFILE_EXCL);
      chmodSync(target, 0o644);
      method = "copy";
    } else {
      symlinkSync(source, target);
      method = "link";
    }
    const size = statSync(target).size;
    files.push({ path: name, method, source, size, sha256: sha256(target) });
  }

  const archive = path.join(cef, "archive.json");
  const manifest = {
    kind: "takosumi.linux-cef-stage@v1",
    fixture: options.fixture,
    inspectionSeam,
    cef: { directory: cef, archive: existsSync(archive) ? JSON.parse(readFileSync(archive, "utf8")) : null },
    bin,
    stage,
    notStaged: skipped,
    files,
  };
  // A new file only: never an existing manifest or a link planted in its place.
  writeFileSync(path.join(options.out, "stage.json"), `${JSON.stringify(manifest, null, 2)}\n`, {
    flag: "wx",
    mode: 0o644,
  });

  const total = files.reduce((sum, file) => sum + file.size, 0);
  const written = files.filter((file) => file.method !== "link").reduce((sum, file) => sum + file.size, 0);
  console.log(`staged ${files.length} files (${mib(total)} MiB, ${mib(written)} MiB written) in ${stage}`);
  for (const line of skipped) console.log(`not staged: ${line}`);
  console.log(`manifest: ${path.join(options.out, "stage.json")}`);
  const [overlay, mapped] = options.fixture
    ? ["tauri.linux-cef-fixture.conf.json", FIXTURE_OUT]
    : ["tauri.linux-cef.conf.json", DEFAULT_OUT];
  if (!samePath(options.out, mapped)) {
    console.log(
      `note: src-tauri/${overlay} maps ${path.relative(REPO, mapped)}/cef/; ` +
        "link that path to this stage or pass an overlay that maps it",
    );
  }
}

function isLink(file) {
  try {
    return lstatSync(file).isSymbolicLink();
  } catch {
    return false;
  }
}

function mib(bytes) {
  return (bytes / (1024 * 1024)).toFixed(1);
}

main();
