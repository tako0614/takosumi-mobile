// The push resource: POST /push/{endpoint_id} (RFC 8030 §5).
//
// Application servers send standard Web Push here: an aes128gcm body (RFC 8291)
// with VAPID (RFC 8292). The relay cannot read the body; it checks the request,
// applies limits, and forwards the body to the device through APNs or FCM.

import type { RelayContext } from "./context.ts";
import { base64UrlDecode, randomId } from "./encoding.ts";
import { BodyTooLarge, problem, readBody } from "./http.ts";
import { MAX_PUSH_BODY_BYTES, type DeliveryOutcome, type Urgency } from "./senders/types.ts";
import { verifyVapid } from "./vapid.ts";

const URGENCIES = new Set<Urgency>(["very-low", "low", "normal", "high"]);
const TOPIC = /^[A-Za-z0-9_-]{1,32}$/u;

// aes128gcm header: salt (16) | record size (4) | key id length (1) | key id.
// For Web Push the key id is the sender's 65-byte P-256 public key, and the
// whole message is one record: at least a 16-byte tag and a delimiter octet.
const AES128GCM_HEADER = 16 + 4 + 1 + 65;
const MIN_BODY = AES128GCM_HEADER + 17;

function aes128gcmProblem(body: Uint8Array): string | null {
  if (body.length < MIN_BODY) return "the body is shorter than an aes128gcm Web Push message";
  const view = new DataView(body.buffer, body.byteOffset, body.byteLength);
  const recordSize = view.getUint32(16);
  const keyIdLength = body[20];
  if (keyIdLength !== 65) return "the aes128gcm key id must be the 65-byte sender public key";
  if (recordSize < 18) return "the aes128gcm record size is too small";
  if (body.length - AES128GCM_HEADER > recordSize) return "a Web Push message must be a single aes128gcm record";
  return null;
}

function parseTtl(header: string | null): number | null {
  if (header === null || !/^\d{1,10}$/u.test(header.trim())) return null;
  return Number(header.trim());
}

export async function handlePush(
  request: Request,
  endpointId: string,
  context: RelayContext,
  origin: string,
): Promise<Response> {
  const target = await context.store.findByEndpoint(endpointId);
  if (!target) return problem(404, "endpoint_not_found", "no such push subscription");

  const applicationServerKey = base64UrlDecode(target.applicationServerKey);
  if (!applicationServerKey) throw new Error("stored application server key is not base64url");
  const now = context.now();
  const vapid = await verifyVapid(request.headers.get("authorization"), {
    audience: origin,
    applicationServerKey,
    now,
  });
  if (!vapid.ok) {
    context.log.info("push_rejected", { status: vapid.failure.status, reason: vapid.failure.code });
    return problem(
      vapid.failure.status,
      vapid.failure.code,
      vapid.failure.message,
      vapid.failure.status === 401 ? { "www-authenticate": "vapid" } : {},
    );
  }

  if (target.device.invalidatedAt !== null) {
    return problem(410, "subscription_gone", "the device behind this subscription is no longer reachable");
  }

  const encoding = request.headers.get("content-encoding")?.trim().toLowerCase();
  if (encoding !== "aes128gcm") {
    return problem(415, "unsupported_content_encoding", "Content-Encoding: aes128gcm is required");
  }
  const requestedTtl = parseTtl(request.headers.get("ttl"));
  if (requestedTtl === null) return problem(400, "ttl_required", "a TTL header with whole seconds is required");
  const ttl = Math.min(requestedTtl, context.limits.maxTtlSeconds);
  const urgencyHeader = request.headers.get("urgency")?.trim().toLowerCase() ?? "normal";
  if (!URGENCIES.has(urgencyHeader as Urgency)) {
    return problem(400, "invalid_urgency", "Urgency is very-low, low, normal or high");
  }
  const urgency = urgencyHeader as Urgency;
  const topicHeader = request.headers.get("topic");
  if (topicHeader !== null && !TOPIC.test(topicHeader)) {
    return problem(400, "invalid_topic", "Topic is at most 32 base64url characters");
  }
  const topic = topicHeader;

  let body: Uint8Array;
  try {
    body = await readBody(request, MAX_PUSH_BODY_BYTES);
  } catch (error) {
    if (error instanceof BodyTooLarge) {
      return problem(413, "payload_too_large", `the encrypted body must be at most ${MAX_PUSH_BODY_BYTES} bytes`);
    }
    throw error;
  }
  const malformed = aes128gcmProblem(body);
  if (malformed) return problem(400, "invalid_aes128gcm", malformed);

  // The device budget is shared by every app on the device, so it counts only
  // messages that passed their own endpoint's limit: one app sending too much
  // must not use up the budget of the others.
  const endpointDecision = await context.rateLimiter.hit(
    [{ scope: "push-endpoint", key: target.endpointId, limit: context.limits.endpointPerMinute, windowSeconds: 60 }],
    now,
  );
  const decision = endpointDecision.allowed
    ? await context.rateLimiter.hit(
        [{ scope: "push-device", key: target.device.deviceId, limit: context.limits.devicePerMinute, windowSeconds: 60 }],
        now,
      )
    : endpointDecision;
  if (!decision.allowed) {
    context.log.info("push_rate_limited", { platform: target.device.platform });
    return problem(429, "rate_limited", "too many messages for this subscription or device", {
      "retry-after": String(decision.retryAfter),
    });
  }

  const sender = context.senders[target.device.platform];
  if (!sender) {
    context.log.error("push_sender_unconfigured", { platform: target.device.platform });
    return problem(503, "delivery_unavailable", "delivery to this device's platform is not configured", {
      "retry-after": "3600",
    });
  }

  const outcome: DeliveryOutcome = await sender.send({
    platform: target.device.platform,
    token: target.device.token,
    apnsEnvironment: target.device.apnsEnvironment,
    subscriptionId: target.subscriptionId,
    body,
    ttl,
    urgency,
    topic,
    now,
  });

  const fields = { platform: target.device.platform, outcome: outcome.kind };
  switch (outcome.kind) {
    case "delivered":
      context.log.info("push_delivered", { ...fields, bytes: body.length, urgency, ttl });
      return new Response(null, {
        status: 201,
        headers: {
          location: `${origin}/message/${randomId(16)}`,
          ttl: String(ttl),
          "cache-control": "no-store",
        },
      });
    case "device-invalid":
      await context.store.invalidateDevice(target.device.deviceId, now);
      context.log.info("push_device_invalid", { ...fields, reason: outcome.reason });
      return problem(410, "subscription_gone", "the device behind this subscription is no longer reachable");
    case "throttled":
      context.log.warn("push_upstream_throttled", { ...fields, reason: outcome.reason });
      return problem(429, "rate_limited", "the platform push service asked to slow down", {
        "retry-after": String(outcome.retryAfter),
      });
    case "unavailable":
      context.log.warn("push_upstream_unavailable", { ...fields, reason: outcome.reason });
      return problem(503, "delivery_unavailable", "the platform push service is unavailable", {
        "retry-after": String(outcome.retryAfter ?? 60),
      });
    case "failed":
      context.log.error("push_upstream_failed", { ...fields, reason: outcome.reason });
      return problem(502, "delivery_failed", "the platform push service did not accept the message");
  }
}
