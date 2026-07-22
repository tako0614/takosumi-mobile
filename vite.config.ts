import solid from "vite-plugin-solid";
import { createTauriMobileViteConfig } from "@takosjp/mobile-kit/vite";

const devPort = 1450;

export default createTauriMobileViteConfig({
  devPort,
  importMetaUrl: import.meta.url,
  resolveMobileKitFromPackage: true,
  plugins: [solid()],
});
