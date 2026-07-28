import { invoke } from "@tauri-apps/api/core";
import { appDataDir, join } from "@tauri-apps/api/path";
import {
  checkPermissions,
  Format,
  requestPermissions,
  scan,
} from "@tauri-apps/plugin-barcode-scanner";
import { authenticate } from "@tauri-apps/plugin-biometric";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { getCurrent, onOpenUrl } from "@tauri-apps/plugin-deep-link";
import { fetch as tauriFetch } from "@tauri-apps/plugin-http";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { openUrl } from "@tauri-apps/plugin-opener";
import { platform } from "@tauri-apps/plugin-os";
import { load } from "@tauri-apps/plugin-store";
import { Stronghold } from "@tauri-apps/plugin-stronghold";
import {
  createTauriMobileDefaultProductBridge,
  detectTauriRuntime,
  isTauriMobilePlatform,
  type NativeBridge,
} from "@takosjp/mobile-kit";
import { productAdapter } from "./product.ts";

export function isUnsupportedTauriDesktop(): boolean {
  return detectTauriRuntime() && !isTauriMobilePlatform(platform());
}

export function createProductNativeBridge(): NativeBridge {
  if (isUnsupportedTauriDesktop()) {
    throw new Error("Takosumi Mobile supports only iOS and Android.");
  }
  if (detectTauriRuntime()) globalThis.fetch = tauriFetch as typeof fetch;
  const opener = { openUrl };
  return createTauriMobileDefaultProductBridge({
    productAdapter,
    keychainService: "com.takosumi",
    invoke,
    path: { appDataDir, join },
    deepLink: { getCurrent, onOpenUrl },
    opener,
    store: { load },
    stronghold: Stronghold,
    platform: { platform },
    notification: { isPermissionGranted, requestPermission, sendNotification },
    barcodeScanner: {
      scan,
      qrCodeFormat: Format.QRCode,
      checkPermissions,
      requestPermissions,
    },
    biometric: { authenticate },
    clipboard: { writeText },
  });
}
