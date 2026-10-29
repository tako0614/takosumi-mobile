import { type AppScreenIcon, safeIconUrl } from "./app-screens.ts";

/**
 * Icons for launcher tiles, Android task descriptions and home-screen
 * shortcuts. Image icons are fetched with the shell's `fetch` (Tauri's HTTP
 * plugin on devices: no webview cookies, no CORS), size-checked, sniffed and
 * turned into a `data:` URL, which the shell's CSP allows in `<img>`. Native
 * surfaces need PNG, so SVG and glyph icons are drawn onto a canvas and
 * exported as PNG here; the native plugin never parses SVG.
 */

export const ICON_MAX_BYTES = 256 * 1024;
export const NATIVE_ICON_PIXELS = 192;
const BRAND_BACKGROUND = "#f5483d";

export type IconFormat = "png" | "jpeg" | "gif" | "webp" | "ico" | "svg";

const FORMAT_MIME: Readonly<Record<IconFormat, string>> = {
  png: "image/png",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  ico: "image/x-icon",
  svg: "image/svg+xml",
};

/** Identify the image by its bytes; SVG also needs an SVG or XML-ish type. */
export function sniffIconFormat(
  contentType: string | null,
  bytes: Uint8Array,
): IconFormat | undefined {
  if (startsWith(bytes, [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])) {
    return "png";
  }
  if (startsWith(bytes, [0xff, 0xd8, 0xff])) return "jpeg";
  if (startsWith(bytes, [0x47, 0x49, 0x46, 0x38])) return "gif";
  if (
    startsWith(bytes, [0x52, 0x49, 0x46, 0x46]) &&
    bytes.length >= 12 &&
    String.fromCharCode(...bytes.slice(8, 12)) === "WEBP"
  ) {
    return "webp";
  }
  if (startsWith(bytes, [0x00, 0x00, 0x01, 0x00])) return "ico";
  const type = (contentType ?? "").split(";")[0]!.trim().toLowerCase();
  if (
    type === "image/svg+xml" ||
    type === "text/xml" ||
    type === "application/xml" ||
    type === "text/plain" ||
    type === ""
  ) {
    const head = new TextDecoder("utf-8", { fatal: false })
      .decode(bytes.slice(0, 1024))
      .replace(/^﻿/u, "")
      .trimStart()
      .toLowerCase();
    if (
      head.startsWith("<svg") ||
      ((head.startsWith("<?xml") || head.startsWith("<!--")) &&
        head.includes("<svg"))
    ) {
      return "svg";
    }
  }
  return undefined;
}

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunk = 0x8000;
  for (let offset = 0; offset < bytes.length; offset += chunk) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunk));
  }
  return btoa(binary);
}

export function iconDataUrl(format: IconFormat, bytes: Uint8Array): string {
  return `data:${FORMAT_MIME[format]};base64,${bytesToBase64(bytes)}`;
}

/**
 * Fetch an icon image as a `data:` URL. Returns `undefined` for anything that
 * is not a small https image; icons are decoration and never block the home.
 */
export async function fetchIconDataUrl(
  rawUrl: string,
  fetchImpl: typeof fetch = fetch,
): Promise<string | undefined> {
  const url = safeIconUrl(rawUrl);
  if (!url) return undefined;
  let response: Response;
  try {
    response = await fetchImpl(url, {
      credentials: "omit",
      headers: { accept: "image/*" },
    });
  } catch {
    return undefined;
  }
  if (!response.ok) return undefined;
  if (response.url && !safeIconUrl(response.url)) return undefined;
  const declared = Number(response.headers.get("content-length"));
  if (Number.isFinite(declared) && declared > ICON_MAX_BYTES) return undefined;
  let bytes: Uint8Array;
  try {
    bytes = new Uint8Array(await response.arrayBuffer());
  } catch {
    return undefined;
  }
  if (bytes.length === 0 || bytes.length > ICON_MAX_BYTES) return undefined;
  const format = sniffIconFormat(response.headers.get("content-type"), bytes);
  return format ? iconDataUrl(format, bytes) : undefined;
}

export type NativeIconSource =
  | { readonly kind: "data-url"; readonly dataUrl: string }
  | { readonly kind: "glyph"; readonly glyph: string };

/** What to draw for native surfaces: the loaded image, else a glyph. */
export function nativeIconSource(
  icon: AppScreenIcon | undefined,
  loadedDataUrl: string | undefined,
  fallbackGlyph: string,
): NativeIconSource {
  if (icon?.kind === "image" && loadedDataUrl) {
    return { kind: "data-url", dataUrl: loadedDataUrl };
  }
  if (icon?.kind === "glyph") return { kind: "glyph", glyph: icon.glyph };
  return { kind: "glyph", glyph: fallbackGlyph };
}

/**
 * Draw the icon as a square PNG and return it as base64 without the `data:`
 * prefix. Needs a DOM canvas; returns `undefined` elsewhere or on failure.
 */
export async function rasterizeIconPng(
  source: NativeIconSource,
  size = NATIVE_ICON_PIXELS,
): Promise<string | undefined> {
  if (typeof document === "undefined") return undefined;
  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  const context = canvas.getContext("2d");
  if (!context) return undefined;
  try {
    if (source.kind === "data-url") {
      const image = await loadImage(source.dataUrl);
      context.fillStyle = "#ffffff";
      context.fillRect(0, 0, size, size);
      const scale = Math.min(
        size / (image.naturalWidth || size),
        size / (image.naturalHeight || size),
      );
      const width = (image.naturalWidth || size) * scale;
      const height = (image.naturalHeight || size) * scale;
      context.drawImage(image, (size - width) / 2, (size - height) / 2, width, height);
    } else {
      context.fillStyle = BRAND_BACKGROUND;
      context.fillRect(0, 0, size, size);
      context.fillStyle = "#ffffff";
      context.textAlign = "center";
      context.textBaseline = "middle";
      const glyph = [...source.glyph];
      const fontSize = glyph.length <= 2 ? size * 0.5 : size * 0.3;
      context.font = `800 ${Math.round(fontSize)}px system-ui, sans-serif`;
      context.fillText(source.glyph, size / 2, size / 2 + size * 0.02, size * 0.9);
    }
    const png = canvas.toDataURL("image/png");
    const prefix = "data:image/png;base64,";
    return png.startsWith(prefix) ? png.slice(prefix.length) : undefined;
  } catch {
    return undefined;
  }
}

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.decoding = "async";
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error("icon image could not be decoded"));
    image.src = src;
  });
}

function startsWith(bytes: Uint8Array, signature: readonly number[]): boolean {
  return (
    bytes.length >= signature.length &&
    signature.every((byte, index) => bytes[index] === byte)
  );
}
