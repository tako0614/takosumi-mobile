// The pure parts of scripts/stage-windows-cef.mjs (W6), kept in their own
// module so tests import them without running the staging script: which
// DLLs a PE file imports at load time and which names it exports, read as
// the Windows loader reads them, and which DLLs a clean Windows provides.

/**
 * Windows system DLLs a staged file may import at load time (lower case):
 * exactly the union of what the staged CEF 154 runtime, browser and guard
 * import (the `imports` of run 42355214657's product and fixture stage
 * manifests), all present on every Windows 10/11 edition. Not the Visual C++
 * runtime (vcruntime140*.dll, msvcp140*.dll and the like), which a clean
 * Windows lacks, and not Media Foundation (mf*.dll, evr.dll), which the N
 * and KN editions lack. A new CEF importing anything else stops the stage
 * until that DLL is checked and added here.
 */
export const SYSTEM_DLLS = new Set([
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

/**
 * API set names (api-ms-win-*, ext-ms-win-*) resolve to system DLLs through
 * the loader's schema. Accepted by prefix: a set newer than the oldest
 * supported Windows, or an ext-ms-win-* set an edition does not provide,
 * would not be caught here.
 */
export const API_SET_PREFIXES = ["api-ms-win-", "ext-ms-win-"];

/** Whether a load-time import is a Windows system DLL (see above). */
export function systemDll(name) {
  const lower = name.toLowerCase();
  return SYSTEM_DLLS.has(lower) || API_SET_PREFIXES.some((prefix) => lower.startsWith(prefix));
}

/**
 * A PE image read through `read(offset, length)`, which returns a Buffer
 * (shorter at the end of the file), as the loader maps it: `directory(index)`
 * is a data directory's RVA (0 when absent) and `mapped(rva, length)` reads
 * the mapped image, where a section's file data starts at its
 * PointerToRawData rounded down to 0x200 and its bytes past SizeOfRawData
 * read as zero, whatever the file holds there. An image without an optional
 * header or with an unknown one throws; the section table is read on first
 * use.
 */
function peImage(read) {
  const dos = read(0, 64);
  if (dos.length < 64 || dos.toString("latin1", 0, 2) !== "MZ") throw new Error("no MZ header");
  const pe = dos.readUInt32LE(0x3c);
  const coff = read(pe, 24);
  if (coff.length < 24 || coff.toString("latin1", 0, 4) !== "PE\0\0") throw new Error("no PE header");
  const sectionCount = coff.readUInt16LE(6);
  const optionalSize = coff.readUInt16LE(20);
  if (optionalSize === 0) throw new Error("no optional header");
  const optional = read(pe + 24, optionalSize);
  if (optional.length < optionalSize) throw new Error("truncated optional header");
  const magic = optional.readUInt16LE(0);
  // NumberOfRvaAndSizes, then the data directories (PE32+ or PE32).
  const directories = magic === 0x20b ? 112 : magic === 0x10b ? 96 : -1;
  if (directories < 0 || optionalSize < directories) throw new Error(`unknown optional header 0x${magic.toString(16)}`);
  const count = optional.readUInt32LE(directories - 4);
  let sections;
  const sectionTable = () => {
    if (sections) return sections;
    const table = read(pe + 24 + optionalSize, sectionCount * 40);
    if (table.length < sectionCount * 40) throw new Error("truncated section table");
    sections = [];
    for (let index = 0; index < sectionCount; index += 1) {
      const base = index * 40;
      sections.push({
        virtualSize: table.readUInt32LE(base + 8),
        virtualAddress: table.readUInt32LE(base + 12),
        rawSize: table.readUInt32LE(base + 16),
        rawOffset: table.readUInt32LE(base + 20) & ~0x1ff,
      });
    }
    return sections;
  };
  return {
    directory(index) {
      if (count < index + 1 || optionalSize < directories + 8 * (index + 1)) return 0;
      return optional.readUInt32LE(directories + 8 * index);
    },
    // `length` bytes of the mapped image at `rva`, zero past the section's file data.
    mapped(rva, length) {
      const section = sectionTable().find(
        ({ virtualAddress, virtualSize, rawSize }) =>
          rva >= virtualAddress && rva < virtualAddress + Math.max(virtualSize, rawSize),
      );
      if (!section) throw new Error(`RVA 0x${rva.toString(16)} lies in no section`);
      const within = rva - section.virtualAddress;
      const bytes = Buffer.alloc(length);
      const available = Math.max(0, Math.min(length, section.rawSize - within));
      if (available > 0) read(section.rawOffset + within, available).copy(bytes);
      return bytes;
    },
  };
}

/** A NUL-terminated name of at most `limit` bytes at `rva`; throws when empty or unterminated. */
function mappedName(image, rva, limit, what) {
  const raw = image.mapped(rva, limit);
  const end = raw.indexOf(0);
  if (end <= 0) throw new Error(`unterminated or empty ${what} name`);
  return raw.toString("latin1", 0, end);
}

/**
 * The DLL names a PE file imports at load time (its import directory; the
 * delay-loaded ones are loaded on first use and not listed), read through
 * `read(offset, length)` as the loader maps the image (see peImage). An
 * image without an optional header, with an unknown one or with a malformed
 * import directory throws; one with no import directory imports nothing.
 */
export function peImports(read) {
  const image = peImage(read);
  const importRva = image.directory(1);
  if (importRva === 0) return [];
  const names = [];
  for (let index = 0; index < 4096; index += 1) {
    const descriptor = image.mapped(importRva + index * 20, 20);
    if (descriptor.every((byte) => byte === 0)) return names;
    names.push(mappedName(image, descriptor.readUInt32LE(12), 260, "import"));
  }
  throw new Error("import directory without an end");
}

/**
 * The names a PE file exports (its export directory's name table, each
 * name's function RVA non-zero), read like peImports. An image with no
 * export directory exports nothing; a malformed one throws. W7: the client
 * DLL CEF's bootstrap loads must export `RunWinMain`.
 */
export function peExports(read) {
  const image = peImage(read);
  const exportRva = image.directory(0);
  if (exportRva === 0) return [];
  const header = image.mapped(exportRva, 40);
  const functions = header.readUInt32LE(20);
  const count = header.readUInt32LE(24);
  if (count > 65536 || functions > 65536) throw new Error("export directory too large");
  if (count === 0) return [];
  const addresses = image.mapped(header.readUInt32LE(28), functions * 4);
  const names = image.mapped(header.readUInt32LE(32), count * 4);
  const ordinals = image.mapped(header.readUInt32LE(36), count * 2);
  const exported = [];
  for (let index = 0; index < count; index += 1) {
    const ordinal = ordinals.readUInt16LE(index * 2);
    if (ordinal >= functions || addresses.readUInt32LE(ordinal * 4) === 0) {
      throw new Error(`export name ${index} names no function`);
    }
    exported.push(mappedName(image, names.readUInt32LE(index * 4), 4096, "export"));
  }
  return exported;
}

/**
 * CEF's own BSD license, from the comment that opens one of its public
 * headers (`include/cef_base.h`): the leading `//` lines, without the
 * comment marks, or null when they do not hold that license.
 */
export function cefLicenseFromHeader(text) {
  const lines = [];
  for (const line of text.split(/\r?\n/)) {
    if (!line.startsWith("//")) break;
    lines.push(line.replace(/^\/\/ ?/, ""));
  }
  const notice = `${lines.join("\n").trim()}\n`;
  return notice.includes("Marshall A. Greenblatt") && notice.includes("Redistribution and use in source and binary forms")
    ? notice
    : null;
}
