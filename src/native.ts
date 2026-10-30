import { addPluginListener, invoke } from "@tauri-apps/api/core";
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
import {
  type AppScreenInvoke,
  type AppScreenListen,
  type AppScreenOpener,
  createAppScreenOpener,
} from "./app-screen-native.ts";
import { productAdapter } from "./product.ts";

export function isUnsupportedTauriDesktop(): boolean {
  return detectTauriRuntime() && !isTauriMobilePlatform(platform());
}

/** `ios`, `android`, or `web` for the browser preview. */
export function productPlatform(): string {
  return detectTauriRuntime() && isTauriMobilePlatform(platform()) ? platform() : "web";
}

/**
 * App screens open in the product's native webview plugin on iOS/Android and
 * in the external browser in the web preview.
 */
export function createProductAppScreenOpener(
  bridge: Pick<NativeBridge, "openExternalUrl">,
): AppScreenOpener {
  const mobile = productPlatform() !== "web";
  return createAppScreenOpener({
    ...(mobile
      ? {
          invoke: invoke as AppScreenInvoke,
          listen: addPluginListener as unknown as AppScreenListen,
        }
      : {}),
    platform: productPlatform(),
    openExternalUrl: (url) => bridge.openExternalUrl(url),
  });
}

/**
 * Store pages (App Store, Google Play, F-Droid) through the system, so the
 * store app takes its own links. `NativeBridge.openExternalUrl` would prefer
 * the in-app browser on phones. Web pages go to `AppScreenOpener.openWeb`.
 */
export function createProductStoreOpener(
  bridge: Pick<NativeBridge, "openExternalUrl">,
): (url: string) => Promise<void> {
  if (productPlatform() === "web") return (url) => bridge.openExternalUrl(url);
  return async (url) => await openUrl(url);
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
