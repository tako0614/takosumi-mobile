// The device API. Only Takosumi Mobile calls it.
//
//   POST   /v1/devices                                   register, returns a device secret
//   PUT    /v1/devices/{device_id}                       replace the APNs/FCM token
//   DELETE /v1/devices/{device_id}                       remove the device and its subscriptions
//   POST   /v1/devices/{device_id}/subscriptions         issue a push endpoint
//   DELETE /v1/devices/{device_id}/subscriptions/{id}    remove one subscription
//
// Everything after registration needs `Authorization: Bearer <device_secret>`.

import type { RelayContext } from "./context.ts";
import { base64UrlDecode, base64UrlEncode, constantTimeEqual, randomId, sha256Hex, utf8 } from "./encoding.ts";
import { BodyTooLarge, json, noContent, problem, readJsonObject } from "./http.ts";
import type { ApnsEnvironment, DeviceRecord, Platform } from "./store.ts";
import { importApplicationServerKey } from "./vapid.ts";

const APNS_TOKEN = /^(?:[0-9a-f]{2}){16,100}$/iu;
const FCM_TOKEN = /^[A-Za-z0-9_:-]{16,4096}$/u;

interface TokenFields {
  token: string;
  apnsEnvironment: ApnsEnvironment | null;
}

function parseToken(platform: Platform, body: Record<string, unknown>): TokenFields | Response {
  const token = body.token;
  if (typeof token !== "string") return problem(400, "invalid_token", "token is required");
  if (platform === "apns") {
    if (!APNS_TOKEN.test(token)) return problem(400, "invalid_token", "an APNs token is hex");
    const environment = body.apns_environment ?? "production";
    if (environment !== "production" && environment !== "development") {
      return problem(400, "invalid_apns_environment", "apns_environment is production or development");
    }
    return { token: token.toLowerCase(), apnsEnvironment: environment };
  }
  if (!FCM_TOKEN.test(token)) return problem(400, "invalid_token", "not an FCM registration token");
  if (body.apns_environment !== undefined) {
    return problem(400, "invalid_apns_environment", "apns_environment applies only to apns devices");
  }
  return { token, apnsEnvironment: null };
}

async function readBodyOrProblem(request: Request): Promise<Record<string, unknown> | Response> {
  try {
    const body = await readJsonObject(request);
    return body ?? problem(400, "invalid_json", "the body must be a JSON object");
  } catch (error) {
    if (error instanceof BodyTooLarge) return problem(413, "body_too_large", "the body is too large");
    throw error;
  }
}

async function clientKey(request: Request): Promise<string> {
  const address = request.headers.get("cf-connecting-ip") ?? "unknown";
  return (await sha256Hex(`client:${address}`)).slice(0, 32);
}

function rateLimited(retryAfter: number): Response {
  return problem(429, "rate_limited", "too many requests", { "retry-after": String(retryAfter) });
}

const unauthorized = () =>
  problem(401, "unauthorized", "a valid device secret is required", { "www-authenticate": "Bearer" });

/**
 * Resolves the device and checks its secret. An unknown device and a wrong
 * secret look the same to the caller; either way it registers again.
 */
async function authenticate(
  request: Request,
  deviceId: string,
  context: RelayContext,
): Promise<DeviceRecord | Response> {
  const header = request.headers.get("authorization") ?? "";
  const match = header.match(/^Bearer\s+([A-Za-z0-9_-]{16,128})$/u);
  const presented = await sha256Hex(match?.[1] ?? "");
  const device = await context.store.getDevice(deviceId);
  const expected = device?.secretHash ?? "0".repeat(64);
  if (!match || !device || !constantTimeEqual(utf8(presented), utf8(expected))) {
    return unauthorized();
  }
  return device;
}

export async function registerDevice(request: Request, context: RelayContext): Promise<Response> {
  const now = context.now();
  const decision = await context.rateLimiter.hit(
    [{ scope: "register", key: await clientKey(request), limit: context.limits.registrationsPerHour, windowSeconds: 3600 }],
    now,
  );
  if (!decision.allowed) return rateLimited(decision.retryAfter);

  const body = await readBodyOrProblem(request);
  if (body instanceof Response) return body;
  const platform = body.platform;
  if (platform !== "apns" && platform !== "fcm") {
    return problem(400, "invalid_platform", "platform is apns or fcm");
  }
  const fields = parseToken(platform, body);
  if (fields instanceof Response) return fields;

  const deviceId = randomId(16);
  const deviceSecret = randomId(32);
  await context.store.createDevice({
    deviceId,
    secretHash: await sha256Hex(deviceSecret),
    platform,
    token: fields.token,
    apnsEnvironment: fields.apnsEnvironment,
    createdAt: now,
  });
  context.log.info("device_registered", { platform });
  return json(201, { device_id: deviceId, device_secret: deviceSecret });
}

export async function updateDevice(request: Request, deviceId: string, context: RelayContext): Promise<Response> {
  const device = await authenticate(request, deviceId, context);
  if (device instanceof Response) return device;
  const body = await readBodyOrProblem(request);
  if (body instanceof Response) return body;
  if (body.platform !== undefined && body.platform !== device.platform) {
    return problem(400, "platform_immutable", "a device cannot change platform; register a new device");
  }
  const fields = parseToken(device.platform, body);
  if (fields instanceof Response) return fields;
  await context.store.updateDeviceToken(deviceId, fields.token, fields.apnsEnvironment, context.now());
  context.log.info("device_token_updated", { platform: device.platform, revived: device.invalidatedAt !== null });
  return noContent();
}

export async function deleteDevice(request: Request, deviceId: string, context: RelayContext): Promise<Response> {
  const device = await authenticate(request, deviceId, context);
  if (device instanceof Response) return device;
  await context.store.deleteDevice(deviceId);
  context.log.info("device_deleted", { platform: device.platform });
  return noContent();
}

export async function createSubscription(
  request: Request,
  deviceId: string,
  context: RelayContext,
  origin: string,
): Promise<Response> {
  const device = await authenticate(request, deviceId, context);
  if (device instanceof Response) return device;
  const now = context.now();
  const decision = await context.rateLimiter.hit(
    [{ scope: "subscribe", key: deviceId, limit: context.limits.subscriptionsPerHour, windowSeconds: 3600 }],
    now,
  );
  if (!decision.allowed) return rateLimited(decision.retryAfter);

  const body = await readBodyOrProblem(request);
  if (body instanceof Response) return body;
  const rawKey =
    typeof body.application_server_key === "string" ? base64UrlDecode(body.application_server_key) : null;
  if (!rawKey || !(await importApplicationServerKey(rawKey))) {
    return problem(
      400,
      "invalid_application_server_key",
      "application_server_key is a base64url uncompressed P-256 public key",
    );
  }
  if (device.invalidatedAt !== null) {
    return problem(409, "device_token_invalid", "the push service rejected this device's token; update it first");
  }
  if ((await context.store.countSubscriptions(deviceId)) >= context.limits.subscriptionsPerDevice) {
    return problem(409, "subscription_limit", "this device holds the maximum number of subscriptions");
  }

  const subscriptionId = randomId(16);
  const endpointId = randomId(32);
  await context.store.createSubscription({
    subscriptionId,
    deviceId,
    endpointId,
    applicationServerKey: base64UrlEncode(rawKey),
    createdAt: now,
  });
  context.log.info("subscription_created", { platform: device.platform });
  return json(201, { subscription_id: subscriptionId, endpoint: `${origin}/push/${endpointId}` });
}

export async function deleteSubscription(
  request: Request,
  deviceId: string,
  subscriptionId: string,
  context: RelayContext,
): Promise<Response> {
  const device = await authenticate(request, deviceId, context);
  if (device instanceof Response) return device;
  if (!(await context.store.deleteSubscription(deviceId, subscriptionId))) {
    return problem(404, "subscription_not_found", "this device holds no such subscription");
  }
  context.log.info("subscription_deleted", { platform: device.platform });
  return noContent();
}
