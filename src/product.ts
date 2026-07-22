import type { MobileProductAdapter } from "@takosjp/mobile-kit";

export const productAdapter: MobileProductAdapter = {
  product: "takosumi",
  appName: "Takosumi",
  // Reads inside Japanese status sentences, so the noun is Japanese too.
  hostNoun: "Takosumi ホスト",
  urlPlaceholder: "https://app.takosumi.com",
  primaryActionLabel: "Takosumiにつなぐ",
  // Single Takosumi-red accent, kept equal to `--tg-accent` in
  // `takosumi/dashboard/src/styles/tokens.css`.
  accentColor: "#f5483d",
  mobileScheme: "takosumi",
  oidcScopes: [
    "openid",
    "profile",
    "offline_access",
    "capsules:read",
    "capsules:write",
  ],
};
