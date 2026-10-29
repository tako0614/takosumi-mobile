import type { RelayContext } from "./context.ts";
import { createSubscription, deleteDevice, deleteSubscription, registerDevice, updateDevice } from "./devices.ts";
import { problem, json } from "./http.ts";
import { errorKind } from "./log.ts";
import { handlePush } from "./push.ts";

export const SERVICE_NAME = "takosumi-push-relay";

const ID = "[A-Za-z0-9_-]{16,64}";
const ROUTES = {
  devices: /^\/v1\/devices$/u,
  device: new RegExp(`^/v1/devices/(${ID})$`, "u"),
  subscriptions: new RegExp(`^/v1/devices/(${ID})/subscriptions$`, "u"),
  subscription: new RegExp(`^/v1/devices/(${ID})/subscriptions/(${ID})$`, "u"),
  push: new RegExp(`^/push/(${ID})$`, "u"),
  message: new RegExp(`^/message/(${ID})$`, "u"),
};

const methodNotAllowed = (allow: string) =>
  problem(405, "method_not_allowed", `use ${allow}`, { allow });

async function route(request: Request, context: RelayContext): Promise<Response> {
  const url = new URL(request.url);
  const origin = context.origin ?? url.origin;
  const path = url.pathname;
  const method = request.method.toUpperCase();
  let match: RegExpMatchArray | null;

  if (path === "/" || path === "/healthz") {
    if (method !== "GET" && method !== "HEAD") return methodNotAllowed("GET");
    return json(200, { service: SERVICE_NAME, status: "ok" });
  }
  if ((match = path.match(ROUTES.push))) {
    if (method !== "POST") return methodNotAllowed("POST");
    return handlePush(request, match[1] as string, context, origin);
  }
  if (ROUTES.devices.test(path)) {
    if (method !== "POST") return methodNotAllowed("POST");
    return registerDevice(request, context);
  }
  if ((match = path.match(ROUTES.device))) {
    if (method === "PUT") return updateDevice(request, match[1] as string, context);
    if (method === "DELETE") return deleteDevice(request, match[1] as string, context);
    return methodNotAllowed("PUT, DELETE");
  }
  if ((match = path.match(ROUTES.subscriptions))) {
    if (method !== "POST") return methodNotAllowed("POST");
    return createSubscription(request, match[1] as string, context, origin);
  }
  if ((match = path.match(ROUTES.subscription))) {
    if (method !== "DELETE") return methodNotAllowed("DELETE");
    return deleteSubscription(request, match[1] as string, match[2] as string, context);
  }
  if (ROUTES.message.test(path)) {
    // RFC 8030 names a message resource in Location. Messages are handed to
    // APNs/FCM at once and not retained, so there is nothing to read or cancel.
    return problem(404, "message_not_retained", "push messages are not retained by this relay");
  }
  return problem(404, "not_found", "no such resource");
}

export function createRelay(context: RelayContext) {
  return {
    async fetch(request: Request): Promise<Response> {
      try {
        return await route(request, context);
      } catch (error) {
        context.log.error("request_failed", { kind: errorKind(error) });
        return problem(500, "internal_error", "the relay failed to handle the request");
      }
    },
    async scheduled(): Promise<void> {
      const now = context.now();
      await context.store.purge(
        now - context.limits.invalidRetentionSeconds,
        now - context.limits.inactiveDeviceSeconds,
        now - 2 * 3600,
      );
      context.log.info("purged", {});
    },
  };
}
