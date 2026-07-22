import { expect, test } from "bun:test";
import { discoverHost, type FetchLike } from "@takosjp/mobile-kit";

test("Takosumi Mobile discovers the host-published PKCE client", async () => {
  const fetcher: FetchLike = async (input) => {
    const url = new URL(String(input));
    if (url.pathname === "/.well-known/takosumi") {
      return Response.json({
        product: "takosumi",
        name: "Takosumi",
        oidcClientId: "takosumi-mobile-operator",
        auth: { oidc: true, password: false },
        endpoints: {
          api: "https://operator.example/api",
          capabilities: "https://operator.example/v1/capabilities",
          oidc_issuer: "https://operator.example",
        },
      });
    }
    if (url.pathname === "/v1/capabilities") {
      return Response.json({ identity: { oidc_issuer: true } });
    }
    throw new Error(`Unexpected discovery path: ${url.pathname}`);
  };

  const discovery = await discoverHost({
    hostUrl: "https://operator.example",
    expectedProduct: "takosumi",
    fetch: fetcher,
  });

  expect(discovery.detectedProduct).toBe("takosumi");
  expect(discovery.oidcClientId).toBe("takosumi-mobile-operator");
  expect(discovery.oidcIssuer).toBe("https://operator.example");
  expect(discovery.authMethods).toEqual({ oidc: true, password: false });
});
