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
import { limit, secondsLeft } from "./rate-limit.ts";
import type { ApnsEnvironment, DeviceRecord, Platform } from "./store.ts";
import { importApplicationServerKey } from "./vapid.ts";

const APNS_TOKEN = /^[0-9a-f]{64}$/iu;
const FCM_TOKEN = /^[A-Za-z0-9_:-]{16,1024}$/u;

interface TokenFields {
  token: string;
  apnsEnvironment: ApnsEnvironment | null;
}

function parseToken(platform: Platform, body: Record<string, unknown>): TokenFields | Response {
  const token = body.token;
  if (typeof token !== "string") return problem(400, "invalid_token", "token is required");
  if (platform === "apns") {
    if (!APNS_TOKEN.test(token)) return problem(400, "invalid_token", "an APNs token is 64 hex characters");
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

/** The eight groups of an IPv6 address, or null when the text is not IPv6. */
function ipv6Groups(address: string): string[] | null {
  if (!address.includes(":")) return null;
  let text = address.trim().toLowerCase().replace(/^\[|\]$/gu, "").split("%")[0] ?? "";
  // An embedded IPv4 tail (::ffff:192.0.2.1) is two groups.
  const v4 = text.match(/(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/u);
  if (v4) {
    const [a, b, c, d] = v4.slice(1).map(Number) as [number, number, number, number];
    text = `${text.slice(0, v4.index)}${((a << 8) | b).toString(16)}:${((c << 8) | d).toString(16)}`;
  }
  const halves = text.split("::");
  if (halves.length > 2) return null;
  const head = halves[0] ? halves[0].split(":") : [];
  const tail = halves.length === 2 && halves[1] ? halves[1].split(":") : [];
  const missing = 8 - head.length - tail.length;
  if ((halves.length === 1 && missing !== 0) || missing < 0) return null;
  const groups = [...head, ...Array(halves.length === 2 ? missing : 0).fill("0"), ...tail];
  if (groups.length !== 8 || groups.some((group) => !/^[0-9a-f]{1,4}$/u.test(group))) return null;
  return groups.map((group) => group.replace(/^0+(?=.)/u, ""));
}

/**
 * The /64 of an IPv6 address, or null when the text is not IPv6. One
 * subscriber usually holds a whole /64, so counting addresses inside it would
 * let one client register without limit.
 */
export function ipv6Prefix64(address: string): string | null {
  return ipv6Groups(address)?.slice(0, 4).join(":") ?? null;
}

/** The /48 of an IPv6 address: a site's whole allocation (65,536 /64s). */
export function ipv6Prefix48(address: string): string | null {
  return ipv6Groups(address)?.slice(0, 3).join(":") ?? null;
}

const hashed = async (text: string) => (await sha256Hex(`client:${text}`)).slice(0, 32);

/** Rate-limit keys for a client: its IPv4 address, or its IPv6 /64 and /48. */
async function clientKeys(request: Request): Promise<{ client: string; site: string | null }> {
  const address = request.headers.get("cf-connecting-ip") ?? "unknown";
  const prefix64 = ipv6Prefix64(address);
  if (!prefix64) return { client: await hashed(`v4:${address}`), site: null };
  return { client: await hashed(`v6/64:${prefix64}`), site: await hashed(`v6/48:${ipv6Prefix48(address)}`) };
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
  await context.store.touchDevice(deviceId, context.now());
  return device;
}

export async function registerDevice(request: Request, context: RelayContext): Promise<Response> {
  const now = context.now();
  const { limits } = context;
  // Narrowest first: a refused client does not use up the wider budgets.
  // These limits only slow abuse down. The defense before production is WAF
  // rate limiting at Cloudflare and device attestation on registration (App
  // Attest on iOS, Play Integrity on Android); see relay/README.md.
  const keys = await clientKeys(request);
  const perClient = await limit(
    context.counters,
    { scope: "register", key: keys.client, limit: limits.registrationsPerHour, windowSeconds: 3600 },
    now,
  );
  if (!perClient.allowed) return rateLimited(perClient.retryAfter);
  if (keys.site) {
    const perSite = await limit(
      context.counters,
      { scope: "register-48", key: keys.site, limit: limits.registrationsPerHourPer48, windowSeconds: 3600 },
      now,
    );
    if (!perSite.allowed) return rateLimited(perSite.retryAfter);
  }
  // Over all clients: an alert well before a hard ceiling, so that one /32's
  // worth of clients cannot lock everyone else out of registering.
  const [global = 0] = await context.counters.run(
    [{ counter: { bucket: "register-global:all", windowSeconds: 3600 }, increment: true }],
    now,
  );
  if (global === limits.registrationsAlertPerHour + 1) {
    context.log.error("registration_rate_alert", { alert: true, perHour: global });
  }
  if (global > limits.registrationsPerHourGlobal) {
    context.log.warn("registration_global_limit", {});
    return rateLimited(secondsLeft(now, 3600));
  }

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
  // Registering a token that another active device holds supersedes that
  // device: its endpoints answer 410 and the apps behind them subscribe again.
  const superseded = await context.store.createDevice({
    deviceId,
    secretHash: await sha256Hex(deviceSecret),
    platform,
    token: fields.token,
    apnsEnvironment: fields.apnsEnvironment,
    createdAt: now,
  });
  await context.store.pruneInvalidRowsForToken(platform, fields.token, limits.invalidRowsPerToken);
  context.log.info("device_registered", { platform, superseded });
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
  const superseded = await context.store.updateDeviceToken(device, fields.token, fields.apnsEnvironment, context.now());
  await context.store.pruneInvalidRowsForToken(device.platform, fields.token, context.limits.invalidRowsPerToken);
  context.log.info("device_token_updated", {
    platform: device.platform,
    revived: device.invalidatedAt !== null,
    superseded,
  });
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
  const decision = await limit(
    context.counters,
    { scope: "subscribe", key: deviceId, limit: context.limits.subscriptionsPerHour, windowSeconds: 3600 },
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
    return problem(409, "device_token_invalid", "this device's token was rejected or superseded; update it first");
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
