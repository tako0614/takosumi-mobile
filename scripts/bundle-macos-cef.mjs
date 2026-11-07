#!/usr/bin/env node
// Assemble the macOS per-profile browser's app bundle (M1): the browser
// executable, CEF's framework and the five helper apps CEF starts its
// renderer, GPU and utility processes from, in the layout CEF requires on
// macOS (see CEF's GeneralUsage "macOS" and cef-rs's build_util/mac.rs):
//
//   takosumi-cef-browser-process.app/Contents/
//     Info.plist
//     MacOS/takosumi-cef-browser-process
//     Frameworks/Chromium Embedded Framework.framework/   (copied as is)
//     Frameworks/takosumi-cef-browser-process Helper.app/Contents/{Info.plist,MacOS/takosumi-cef-browser-process Helper}
//     Frameworks/takosumi-cef-browser-process Helper (GPU).app/...
//     Frameworks/takosumi-cef-browser-process Helper (Renderer).app/...
//     Frameworks/takosumi-cef-browser-process Helper (Plugin).app/...
//     Frameworks/takosumi-cef-browser-process Helper (Alerts).app/...
//
//   node scripts/bundle-macos-cef.mjs --cef <dir with Chromium Embedded Framework.framework> --bin <build dir> --out <dir> [--fixture]
//
// `--cef` is the distribution cef-dll-sys unpacks into CEF_PATH
// (`<version>/cef_macos_aarch64`: the Release directory, where the framework
// is). `--bin` holds the built takosumi-cef-browser-process and
// takosumi-cef-browser-helper (native-tests/cef-candidate: `cargo build
// --bin takosumi-cef-browser-process --bin takosumi-cef-browser-helper`).
// The bundle is `<out>/takosumi-cef-browser-process.app`; it and
// `<out>/bundle.json` (each file's size, mode and SHA-256, the six
// Info.plist files among them, each symbolic link's target, the
// architecture) must not exist yet: nothing is replaced or removed, and
// every check runs before anything is written.
//
// Every helper app holds a copy of the same helper executable under its
// own name; CEF finds them by the browser executable's name. The framework
// is copied with its symbolic links as links; a link that is absolute,
// dangling or, resolved through every link on its way, leaves the framework
// stops the bundle, as does a file with a setuid, setgid or sticky bit. The browser, the helper and the
// framework's library must be 64-bit Mach-O files of one architecture (the
// framework may be universal). The framework's `Libraries/libcef_sandbox.dylib`
// must be there: each helper enters the seatbelt sandbox from it.
//
// A browser built with the cef-candidate feature `fixture-inspection` (it
// contains the TAKOSUMI_BROWSER_PROCESS_INSPECT seam) is refused unless
// `--fixture` is given.
//
// Nothing is signed here: the Rust linker signs its macOS executables
// ad hoc, CEF's framework comes as distributed, and Developer ID signing
// and notarization are release inputs, not this script's.
//
// Node or Bun. Only file copies: it runs on any OS, so it can be checked
// away from macOS (tests/bundle-macos-cef.test.ts).

import { createHash } from "node:crypto";
import {
  chmodSync,
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
  readlinkSync,
  realpathSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

export const BROWSER = "takosumi-cef-browser-process";
export const HELPER = "takosumi-cef-browser-helper";
export const FRAMEWORK = "Chromium Embedded Framework.framework";
export const FRAMEWORK_BINARY = "Chromium Embedded Framework";
export const SANDBOX_LIBRARY = "Libraries/libcef_sandbox.dylib";
/** The helper apps CEF looks for, by its suffix after "<browser> Helper". */
export const HELPERS = [
  { suffix: "", identifier: "helper" },
  { suffix: " (GPU)", identifier: "helper.gpu" },
  { suffix: " (Renderer)", identifier: "helper.renderer" },
  { suffix: " (Plugin)", identifier: "helper.plugin" },
  { suffix: " (Alerts)", identifier: "helper.alerts" },
];
export const IDENTIFIER = "com.takosumi.cef-browser";
const INSPECTION_SEAM = Buffer.from("TAKOSUMI_BROWSER_PROCESS_INSPECT");
/** Mach-O CPU types (with CPU_ARCH_ABI64). */
export const CPU = { 0x0100000c: "arm64", 0x01000007: "x86_64" };

function usage(message) {
  console.error(`bundle-macos-cef: ${message}`);
  console.error(
    `usage: bundle-macos-cef.mjs --cef <dir with ${FRAMEWORK}> --bin <dir with ${BROWSER} and ${HELPER}> ` +
      "--out <dir> [--fixture]",
  );
  process.exit(2);
}

function parse(argv) {
  const options = { fixture: false };
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (flag === "--fixture") {
      options.fixture = true;
    } else if (flag === "--cef" || flag === "--bin" || flag === "--out") {
      const value = argv[index + 1];
      if (!value) usage(`${flag} needs a directory`);
      options[flag.slice(2)] = value;
      index += 1;
    } else {
      usage(`unknown argument ${flag}`);
    }
  }
  if (!options.cef || !options.bin || !options.out) usage("--cef, --bin and --out are required");
  return options;
}

/**
 * The architectures of a Mach-O file from its first bytes: one for a thin
 * 64-bit file, each slice's for a universal one; null when it is neither.
 */
export function machArchitectures(header) {
  if (header.length < 8) return null;
  if (header.readUInt32LE(0) === 0xfeedfacf) {
    const arch = CPU[header.readUInt32LE(4)];
    return arch ? [arch] : [`cpu 0x${header.readUInt32LE(4).toString(16)}`];
  }
  if (header.readUInt32BE(0) === 0xcafebabe) {
    const count = header.readUInt32BE(4);
    if (count === 0 || count > 16 || header.length < 8 + count * 20) return null;
    const found = [];
    for (let index = 0; index < count; index += 1) {
      const cpu = header.readUInt32BE(8 + index * 20);
      found.push(CPU[cpu] ?? `cpu 0x${cpu.toString(16)}`);
    }
    return found;
  }
  return null;
}

function architecturesOf(file) {
  const handle = openSync(file, "r");
  try {
    const header = Buffer.alloc(4096);
    return machArchitectures(header.subarray(0, readSync(handle, header, 0, header.length, 0)));
  } finally {
    closeSync(handle);
  }
}

/** XML-escaped text for a property list. */
function xml(text) {
  return String(text).replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;");
}

/** A property list from a flat object of strings, booleans and string maps. */
export function plist(entries) {
  const value = (item, indent) => {
    if (typeof item === "boolean") return `${indent}<${item}/>`;
    if (typeof item === "string") return `${indent}<string>${xml(item)}</string>`;
    const inner = Object.entries(item)
      .map(([key, nested]) => `${indent}\t<key>${xml(key)}</key>\n${value(nested, `${indent}\t`)}`)
      .join("\n");
    return `${indent}<dict>\n${inner}\n${indent}</dict>`;
  };
  return (
    '<?xml version="1.0" encoding="UTF-8"?>\n' +
    '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n' +
    '<plist version="1.0">\n' +
    `${value(entries, "")}\n` +
    "</plist>\n"
  );
}

/**
 * The Info.plist of the browser app or one of its helper apps: CEF's and
 * cef-rs's keys (MallocNanoZone off, as CEF asks; helpers are UI elements
 * without a Dock icon), the usage descriptions macOS requires before a page
 * may ask for the camera, microphone, Bluetooth or a passkey.
 */
export function infoPlist(executable, identifier, helper) {
  return plist({
    CFBundleDevelopmentRegion: "en",
    CFBundleDisplayName: executable,
    CFBundleExecutable: executable,
    CFBundleIdentifier: identifier,
    CFBundleInfoDictionaryVersion: "6.0",
    CFBundleName: executable,
    CFBundlePackageType: "APPL",
    CFBundleShortVersionString: "0.0.0",
    CFBundleVersion: "0.0.0",
    LSEnvironment: { MallocNanoZone: "0" },
    LSFileQuarantineEnabled: true,
    LSMinimumSystemVersion: "12.0",
    ...(helper ? { LSUIElement: "1" } : {}),
    NSBluetoothAlwaysUsageDescription: "A site in this browser asks to use Bluetooth.",
    NSCameraUsageDescription: "A site in this browser asks to use the camera.",
    NSHighResolutionCapable: true,
    NSMicrophoneUsageDescription: "A site in this browser asks to use the microphone.",
    NSSupportsAutomaticGraphicsSwitching: true,
    NSWebBrowserPublicKeyCredentialUsageDescription: "A site in this browser asks to use a passkey.",
  });
}

function sha256(file) {
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

function isLink(file) {
  try {
    return lstatSync(file).isSymbolicLink();
  } catch {
    return false;
  }
}

function requireFile(file, what) {
  if (!existsSync(file) || isLink(file) || !statSync(file).isFile()) usage(`${what} not found or not a plain file: ${file}`);
  return file;
}

/**
 * Check the framework tree before anything is written and return what to
 * copy, in order (each directory before its entries). A symbolic link must
 * be relative, resolve (no dangling link) and, resolved on the source side
 * through every link on its way (`realpath`), stay inside the framework:
 * a chain such as `R -> .` with `esc -> R/../x` resolves outside although
 * its text does not. A file with a setuid, setgid or sticky bit is refused;
 * the copied mode is the file's permission bits (`mode & 0o777`).
 */
function scanTree(source, realRoot, prefix, plan) {
  for (const name of readdirSync(source).sort()) {
    const from = path.join(source, name);
    const relative = path.posix.join(prefix, name);
    const info = lstatSync(from);
    if (info.isSymbolicLink()) {
      const link = readlinkSync(from);
      if (path.isAbsolute(link)) usage(`${from} links outside the framework (${link})`);
      let real;
      try {
        real = realpathSync(from);
      } catch {
        usage(`${from} is a dangling link (${link})`);
      }
      if (!(real === realRoot || real.startsWith(realRoot + path.sep))) usage(`${from} links outside the framework (${link})`);
      plan.push({ kind: "link", from, relative, link });
    } else if (info.isDirectory()) {
      plan.push({ kind: "directory", relative });
      scanTree(from, realRoot, relative, plan);
    } else if (info.isFile()) {
      if (info.mode & 0o7000) {
        usage(`${from} has a setuid, setgid or sticky bit (mode ${(info.mode & 0o7777).toString(8)})`);
      }
      plan.push({ kind: "file", from, relative, mode: info.mode & 0o777 });
    } else {
      usage(`${from} is neither a file, a directory nor a link`);
    }
  }
  return plan;
}

/** Copy a checked plan (scanTree) under `base` and record each entry. */
function copyPlan(plan, base, entries) {
  for (const item of plan) {
    const to = path.join(base, ...item.relative.split("/"));
    if (item.kind === "directory") {
      mkdirSync(to);
    } else if (item.kind === "link") {
      symlinkSync(item.link, to);
      entries.push({ path: item.relative, link: item.link });
    } else {
      copyFileSync(item.from, to, constants.COPYFILE_EXCL);
      chmodSync(to, item.mode);
      entries.push({ path: item.relative, size: statSync(to).size, sha256: sha256(to), mode: item.mode.toString(8) });
    }
  }
}

/** Write a new Info.plist and record it. */
function writePlist(file, relative, text, entries) {
  writeFileSync(file, text, { flag: "wx" });
  const info = statSync(file);
  entries.push({ path: relative, size: info.size, sha256: sha256(file), mode: (info.mode & 0o777).toString(8) });
}

function copyExecutable(source, target, relative, entries) {
  copyFileSync(source, target, constants.COPYFILE_EXCL);
  chmodSync(target, 0o755);
  entries.push({ path: relative, size: statSync(target).size, sha256: sha256(target), mode: "755" });
}

function main() {
  const options = parse(process.argv.slice(2));
  const cef = realpathSync(options.cef);
  const bin = realpathSync(options.bin);
  const app = path.join(options.out, `${BROWSER}.app`);
  const manifestPath = path.join(options.out, "bundle.json");
  for (const existing of [app, manifestPath]) {
    if (existsSync(existing) || isLink(existing)) usage(`${existing} already exists; remove it yourself or choose another --out`);
  }

  const browser = requireFile(path.join(bin, BROWSER), `built ${BROWSER}`);
  const helper = requireFile(path.join(bin, HELPER), `built ${HELPER}`);
  const framework = path.join(cef, FRAMEWORK);
  if (!existsSync(framework) || isLink(framework) || !statSync(framework).isDirectory()) {
    usage(`${FRAMEWORK} not found in ${cef}`);
  }
  const library = requireFile(path.join(framework, FRAMEWORK_BINARY), "CEF's framework library");
  requireFile(path.join(framework, SANDBOX_LIBRARY), "CEF's sandbox library");

  const architectures = {};
  for (const [name, file] of [["browser", browser], ["helper", helper], ["framework", library]]) {
    const found = architecturesOf(file);
    if (!found) usage(`${file} is not a 64-bit Mach-O file`);
    architectures[name] = found;
  }
  if (architectures.browser.length !== 1 || architectures.helper.length !== 1) {
    usage(`the browser and the helper must be thin Mach-O files (${architectures.browser}, ${architectures.helper})`);
  }
  const arch = architectures.browser[0];
  if (architectures.helper[0] !== arch || !architectures.framework.includes(arch)) {
    usage(`one architecture is needed: browser ${arch}, helper ${architectures.helper}, framework ${architectures.framework}`);
  }

  const inspectionSeam = readFileSync(browser).includes(INSPECTION_SEAM);
  if (inspectionSeam && !options.fixture) {
    usage(
      `${browser} contains the fixture inspection seam (built with --features fixture-inspection); ` +
        "a product bundle refuses it. Rebuild without the feature, or pass --fixture",
    );
  }

  // Every check of the framework runs before anything is written, so a
  // refusal never leaves a partial bundle.
  const frameworkPlan = scanTree(framework, realpathSync(framework), `Contents/Frameworks/${FRAMEWORK}`, []);

  const entries = [];
  const contents = path.join(app, "Contents");
  mkdirSync(path.join(contents, "MacOS"), { recursive: true });
  mkdirSync(path.join(contents, "Frameworks", FRAMEWORK), { recursive: true });
  writePlist(path.join(contents, "Info.plist"), "Contents/Info.plist", infoPlist(BROWSER, IDENTIFIER, false), entries);
  copyExecutable(browser, path.join(contents, "MacOS", BROWSER), `Contents/MacOS/${BROWSER}`, entries);
  copyPlan(frameworkPlan, app, entries);
  for (const { suffix, identifier } of HELPERS) {
    const name = `${BROWSER} Helper${suffix}`;
    const helperContents = path.join(contents, "Frameworks", `${name}.app`, "Contents");
    mkdirSync(path.join(helperContents, "MacOS"), { recursive: true });
    writePlist(
      path.join(helperContents, "Info.plist"),
      `Contents/Frameworks/${name}.app/Contents/Info.plist`,
      infoPlist(name, `${IDENTIFIER}.${identifier}`, true),
      entries,
    );
    copyExecutable(helper, path.join(helperContents, "MacOS", name), `Contents/Frameworks/${name}.app/Contents/MacOS/${name}`, entries);
  }

  let distribution = null;
  try {
    distribution = JSON.parse(readFileSync(path.join(cef, "archive.json"), "utf8"));
  } catch {
    // Not every distribution directory carries cef-dll-sys's download record.
  }
  const manifest = {
    format: 1,
    bundle: `${BROWSER}.app`,
    identifier: IDENTIFIER,
    architecture: arch,
    fixture: options.fixture,
    inspectionSeam,
    helpers: HELPERS.map(({ suffix }) => `${BROWSER} Helper${suffix}.app`),
    cefDistribution: distribution,
    files: entries,
  };
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, { flag: "wx" });
  console.log(`bundled ${app} (${arch}, ${entries.length} entries${options.fixture ? ", fixture" : ""})`);
}

// Run when started as a script (also through a symbolic link), not when a
// test imports the pure helpers.
const invoked = process.argv[1] ? realpathSync(process.argv[1]) : "";
if (invoked === realpathSync(fileURLToPath(import.meta.url))) main();
