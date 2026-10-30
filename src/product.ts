import type { MobileProductAdapter } from "@takosjp/mobile-kit";

export const productAdapter: MobileProductAdapter = {
  product: "takosumi",
  discoveryProduct: "takosumi",
  strictDiscoveryProduct: true,
  appName: "Takosumi",
  // Reads inside Japanese status sentences, so the noun is Japanese too.
  hostNoun: "Takosumi ホスト",
  urlPlaceholder: "https://app.takosumi.com",
  primaryActionLabel: "Takosumiにつなぐ",
  // Single Takosumi-red accent, kept equal to `--tg-accent` in
  // `takosumi/dashboard/src/styles/tokens.css`.
  accentColor: "#f5483d",
  mobileScheme: "takosumi",
  // Exactly the scopes Takosumi registers for the Takosumi Mobile client
  // (built-in `takosumi-mobile`). The server refuses a request with any scope
  // outside the registration, and `capsules:write` covers Run approval and
  // cancellation.
  oidcScopes: [
    "openid",
    "profile",
    "offline_access",
    "capsules:read",
    "capsules:write",
  ],
  // The issuer's token is an API credential for this same instance. Refuse an
  // instance that names another instance's issuer, which would otherwise
  // receive that instance's token after a genuine consent there.
  requireHostIssuer: true,
};
