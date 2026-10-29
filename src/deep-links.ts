import type { NativeBridge } from "@takosjp/mobile-kit";
import {
  parseAppScreenDeepLink,
  type ParsedAppScreenDeepLink,
} from "./app-screens.ts";

/**
 * Takes `takosumi://app?url=...` links before the shared mobile shell sees
 * them. The shell treats unknown links as a host to connect to; app-screen
 * links instead open the URL in an app screen, without needing the Takosumi
 * session to be unlocked (app screens hold no Takosumi credential).
 * Every other payload (OIDC callback, host routes, connect links) passes
 * through unchanged.
 */
export function withAppScreenDeepLinks(
  bridge: NativeBridge,
  onAppScreenLink: (link: ParsedAppScreenDeepLink) => void,
): NativeBridge {
  // The launch URL stays readable for the life of the process; open it once.
  const handledLaunchPayloads = new Set<string>();
  const onLaunchPayload = bridge.onLaunchPayload;
  return {
    ...bridge,
    async getLaunchPayload() {
      const payload = await bridge.getLaunchPayload();
      if (payload === undefined) return undefined;
      const link = parseAppScreenDeepLink(payload);
      if (!link) return payload;
      if (!handledLaunchPayloads.has(payload)) {
        handledLaunchPayloads.add(payload);
        onAppScreenLink(link);
      }
      return undefined;
    },
    ...(onLaunchPayload
      ? {
          onLaunchPayload: async (handler: (payload: string) => void) =>
            await onLaunchPayload((payload) => {
              const link = parseAppScreenDeepLink(payload);
              if (link) {
                handledLaunchPayloads.add(payload);
                onAppScreenLink(link);
              } else {
                handler(payload);
              }
            }),
        }
      : {}),
  };
}
