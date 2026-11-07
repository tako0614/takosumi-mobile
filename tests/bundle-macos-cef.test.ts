// scripts/bundle-macos-cef.mjs on a synthetic CEF macOS distribution (it
// only copies files, so it runs on any OS), and its pure helpers.
import { afterAll, describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import {
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readlinkSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

const SCRIPT = path.join(import.meta.dir, "..", "scripts", "bundle-macos-cef.mjs");
const bundler = await import(SCRIPT);
const root = mkdtempSync(path.join(tmpdir(), "bundle-macos-cef-"));
afterAll(() => rmSync(root, { recursive: true, force: true }));

const ARM64 = 0x0100000c;
const X86_64 = 0x01000007;

/** A thin 64-bit Mach-O header for `cpu`, then `trailer`. */
function macho(cpu = ARM64, trailer = ""): Buffer {
  const header = Buffer.alloc(32);
  header.writeUInt32LE(0xfeedfacf, 0);
  header.writeUInt32LE(cpu, 4);
  return Buffer.concat([header, Buffer.from(trailer)]);
}

/** A universal (fat) header listing `cpus`. */
function fat(cpus: number[]): Buffer {
  const header = Buffer.alloc(8 + cpus.length * 20);
  header.writeUInt32BE(0xcafebabe, 0);
  header.writeUInt32BE(cpus.length, 4);
  cpus.forEach((cpu, index) => header.writeUInt32BE(cpu, 8 + index * 20));
  return header;
}

let cases = 0;
function distribution(options: { framework?: Buffer; browser?: Buffer; helper?: Buffer; sandbox?: boolean } = {}) {
  const base = path.join(root, `case-${cases++}`);
  const cef = path.join(base, "cef_macos_aarch64");
  const framework = path.join(cef, "Chromium Embedded Framework.framework");
  const bin = path.join(base, "debug");
  mkdirSync(path.join(framework, "Libraries"), { recursive: true });
  mkdirSync(path.join(framework, "Resources", "en.lproj"), { recursive: true });
  mkdirSync(bin, { recursive: true });
  writeFileSync(path.join(framework, "Chromium Embedded Framework"), options.framework ?? macho(ARM64, "framework"));
  if (options.sandbox !== false) writeFileSync(path.join(framework, "Libraries", "libcef_sandbox.dylib"), macho());
  writeFileSync(path.join(framework, "Libraries", "libEGL.dylib"), macho());
  writeFileSync(path.join(framework, "Resources", "icudtl.dat"), "icu");
  writeFileSync(path.join(framework, "Resources", "en.lproj", "locale.pak"), "en");
  // A link inside the framework stays a link.
  symlinkSync("Resources/icudtl.dat", path.join(framework, "icudtl-link.dat"));
  writeFileSync(path.join(cef, "archive.json"), JSON.stringify({ name: "cef_binary_test_macosarm64_minimal.tar.bz2" }));
  writeFileSync(path.join(bin, "takosumi-cef-browser-process"), options.browser ?? macho(ARM64, "browser"), { mode: 0o755 });
  writeFileSync(path.join(bin, "takosumi-cef-browser-helper"), options.helper ?? macho(ARM64, "helper"), { mode: 0o755 });
  return { base, cef, framework, bin, out: path.join(base, "out") };
}

function bundle(args: string[], script = SCRIPT, runtime = process.execPath) {
  return spawnSync(runtime, [script, ...args], { encoding: "utf8" });
}

describe("bundle-macos-cef helpers", () => {
  test("reads thin and universal Mach-O architectures", () => {
    expect(bundler.machArchitectures(macho(ARM64))).toEqual(["arm64"]);
    expect(bundler.machArchitectures(macho(X86_64))).toEqual(["x86_64"]);
    expect(bundler.machArchitectures(fat([X86_64, ARM64]))).toEqual(["x86_64", "arm64"]);
    expect(bundler.machArchitectures(Buffer.from("MZ not a mach-o"))).toBeNull();
    expect(bundler.machArchitectures(Buffer.alloc(4))).toBeNull();
    // A 32-bit Mach-O is not one of them.
    const thin32 = Buffer.alloc(32);
    thin32.writeUInt32LE(0xfeedface, 0);
    expect(bundler.machArchitectures(thin32)).toBeNull();
  });

  test("writes Info.plist with CEF's keys, escaping its text", () => {
    const main = bundler.infoPlist("takosumi-cef-browser-process", "com.takosumi.cef-browser", false);
    expect(main).toContain("<key>CFBundleExecutable</key>\n\t<string>takosumi-cef-browser-process</string>");
    expect(main).toContain("<key>CFBundlePackageType</key>\n\t<string>APPL</string>");
    expect(main).toContain("<key>LSEnvironment</key>\n\t<dict>\n\t\t<key>MallocNanoZone</key>\n\t\t<string>0</string>\n\t</dict>");
    expect(main).toContain("<key>NSHighResolutionCapable</key>\n\t<true/>");
    expect(main).not.toContain("LSUIElement");
    const helper = bundler.infoPlist("takosumi-cef-browser-process Helper (GPU)", "com.takosumi.cef-browser.helper.gpu", true);
    expect(helper).toContain("<key>LSUIElement</key>\n\t<string>1</string>");
    expect(helper).toContain("<string>takosumi-cef-browser-process Helper (GPU)</string>");
    expect(bundler.plist({ a: "<&\">" })).toContain("<string>&lt;&amp;&quot;&gt;</string>");
  });
});

describe("bundle-macos-cef", () => {
  test("assembles the app bundle: browser, framework with its links, five helper apps", () => {
    const { cef, bin, out } = distribution();
    const result = bundle(["--cef", cef, "--bin", bin, "--out", out]);
    expect(result.stderr).toBe("");
    expect(result.status).toBe(0);
    const app = path.join(out, "takosumi-cef-browser-process.app", "Contents");
    expect(readFileSync(path.join(app, "MacOS", "takosumi-cef-browser-process"), "latin1")).toEndWith("browser");
    expect(statSync(path.join(app, "MacOS", "takosumi-cef-browser-process")).mode & 0o777).toBe(0o755);
    expect(readFileSync(path.join(app, "Info.plist"), "utf8")).toContain("<string>com.takosumi.cef-browser</string>");
    const framework = path.join(app, "Frameworks", "Chromium Embedded Framework.framework");
    expect(readFileSync(path.join(framework, "Libraries", "libcef_sandbox.dylib")).length).toBe(32);
    expect(lstatSync(path.join(framework, "icudtl-link.dat")).isSymbolicLink()).toBe(true);
    expect(readlinkSync(path.join(framework, "icudtl-link.dat"))).toBe("Resources/icudtl.dat");
    for (const suffix of ["", " (GPU)", " (Renderer)", " (Plugin)", " (Alerts)"]) {
      const name = `takosumi-cef-browser-process Helper${suffix}`;
      const helper = path.join(app, "Frameworks", `${name}.app`, "Contents");
      expect(readFileSync(path.join(helper, "MacOS", name), "latin1")).toEndWith("helper");
      expect(statSync(path.join(helper, "MacOS", name)).mode & 0o777).toBe(0o755);
      const info = readFileSync(path.join(helper, "Info.plist"), "utf8");
      expect(info).toContain(`<key>CFBundleExecutable</key>\n\t<string>${name}</string>`);
      expect(info).toContain("<key>LSUIElement</key>");
    }
    const manifest = JSON.parse(readFileSync(path.join(out, "bundle.json"), "utf8"));
    expect(manifest.architecture).toBe("arm64");
    expect(manifest.fixture).toBe(false);
    expect(manifest.helpers).toHaveLength(5);
    expect(manifest.cefDistribution.name).toBe("cef_binary_test_macosarm64_minimal.tar.bz2");
    const link = manifest.files.find((file: { path: string }) => file.path.endsWith("icudtl-link.dat"));
    expect(link).toEqual({
      path: "Contents/Frameworks/Chromium Embedded Framework.framework/icudtl-link.dat",
      link: "Resources/icudtl.dat",
    });
    expect(manifest.files.filter((file: { path: string }) => file.path.includes("Helper"))).toHaveLength(5);
    // Never over an existing bundle.
    const again = bundle(["--cef", cef, "--bin", bin, "--out", out]);
    expect(again.status).toBe(2);
    expect(again.stderr).toContain("already exists");
  });

  test("runs its checks when started through a symbolic link, under bun and node", () => {
    for (const runtime of [process.execPath, "node"]) {
      const { cef, bin, out, base } = distribution();
      const link = path.join(base, "bundle-link.mjs");
      symlinkSync(SCRIPT, link);
      const refused = bundle(["--cef", cef, "--bin", bin], link, runtime);
      expect(refused.status).toBe(2);
      expect(refused.stderr).toContain("--cef, --bin and --out are required");
      expect(bundle(["--cef", cef, "--bin", bin, "--out", out], link, runtime).status).toBe(0);
    }
  });

  test("refuses a link that leaves the framework, a missing sandbox library or framework", () => {
    const escaping = distribution();
    symlinkSync("../../debug/takosumi-cef-browser-helper", path.join(escaping.framework, "outside"));
    const refused = bundle(["--cef", escaping.cef, "--bin", escaping.bin, "--out", escaping.out]);
    expect(refused.status).toBe(2);
    expect(refused.stderr).toContain("links outside the framework");
    const absolute = distribution();
    symlinkSync("/etc/hosts", path.join(absolute.framework, "absolute"));
    expect(bundle(["--cef", absolute.cef, "--bin", absolute.bin, "--out", absolute.out]).stderr).toContain(
      "links outside the framework",
    );
    const noSandbox = distribution({ sandbox: false });
    const missing = bundle(["--cef", noSandbox.cef, "--bin", noSandbox.bin, "--out", noSandbox.out]);
    expect(missing.status).toBe(2);
    expect(missing.stderr).toContain("CEF's sandbox library not found");
    const noFramework = distribution();
    rmSync(noFramework.framework, { recursive: true });
    expect(bundle(["--cef", noFramework.cef, "--bin", noFramework.bin, "--out", noFramework.out]).stderr).toContain(
      "Chromium Embedded Framework.framework not found",
    );
  });

  test("needs one architecture and Mach-O files", () => {
    const mixed = distribution({ helper: macho(X86_64, "helper") });
    const refused = bundle(["--cef", mixed.cef, "--bin", mixed.bin, "--out", mixed.out]);
    expect(refused.status).toBe(2);
    expect(refused.stderr).toContain("one architecture is needed");
    const otherFramework = distribution({ framework: macho(X86_64) });
    expect(bundle(["--cef", otherFramework.cef, "--bin", otherFramework.bin, "--out", otherFramework.out]).stderr).toContain(
      "one architecture is needed",
    );
    const universal = distribution({ framework: fat([X86_64, ARM64]) });
    expect(bundle(["--cef", universal.cef, "--bin", universal.bin, "--out", universal.out]).status).toBe(0);
    const notMacho = distribution({ browser: Buffer.from("\x7fELF browser") });
    expect(bundle(["--cef", notMacho.cef, "--bin", notMacho.bin, "--out", notMacho.out]).stderr).toContain(
      "is not a 64-bit Mach-O file",
    );
  });

  test("refuses a fixture browser for a product bundle and accepts it with --fixture", () => {
    const { cef, bin, out } = distribution({ browser: macho(ARM64, "TAKOSUMI_BROWSER_PROCESS_INSPECT") });
    const refused = bundle(["--cef", cef, "--bin", bin, "--out", out]);
    expect(refused.status).toBe(2);
    expect(refused.stderr).toContain("inspection seam");
    const fixture = bundle(["--cef", cef, "--bin", bin, "--out", out, "--fixture"]);
    expect(fixture.status).toBe(0);
    const manifest = JSON.parse(readFileSync(path.join(out, "bundle.json"), "utf8"));
    expect([manifest.fixture, manifest.inspectionSeam]).toEqual([true, true]);
  });
});
