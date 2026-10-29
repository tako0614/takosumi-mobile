// The push resource: POST /push/{endpoint_id} (RFC 8030 §5).
//
// Application servers send standard Web Push here: an aes128gcm body (RFC 8291)
// with VAPID (RFC 8292). The relay cannot read the body; it checks the request,
// applies limits, and forwards the body to the device through APNs or FCM.

import type { RelayContext } from "./context.ts";
import { base64UrlDecode, randomId } from "./encoding.ts";
import { BodyTooLarge, problem, readBody } from "./http.ts";
import { secondsLeft, type WindowCounter } from "./rate-limit.ts";
import { MAX_PUSH_BODY_BYTES, type DeliveryOutcome, type Urgency } from "./senders/types.ts";
import type { Platform, SubscriptionTarget } from "./store.ts";
import { verifyVapid } from "./vapid.ts";

const URGENCIES = new Set<Urgency>(["very-low", "low", "normal", "high"]);
const TOPIC = /^[A-Za-z0-9_-]{1,32}$/u;
const MINUTE = 60;

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

const minute = (bucket: string): WindowCounter => ({ bucket, windowSeconds: MINUTE });

/**
 * Per-endpoint limit, then a per-device ceiling with a fair share.
 *
 * Every endpoint may send `endpointPerMinute`. The device as a whole may send
 * `devicePerMinute`; once that ceiling is reached, an endpoint may still send
 * while it is within its fair share, `devicePerMinute` divided by the device's
 * endpoints that sent this minute. So a few busy apps can use the device budget
 * but cannot starve an app that sends little. Requests refused by a limit do
 * not count against the device.
 */
async function admit(
  context: RelayContext,
  target: SubscriptionTarget,
  now: number,
): Promise<{ allowed: true } | { allowed: false; retryAfter: number }> {
  const { limits, counters } = context;
  const deviceId = target.device.deviceId;
  const [endpointCount = 0, deviceCount = 0, activeBefore = 0] = await counters.run(
    [
      { counter: minute(`push-endpoint:${target.endpointId}`), increment: true },
      { counter: minute(`push-device:${deviceId}`), increment: false },
      { counter: minute(`push-device-active:${deviceId}`), increment: false },
    ],
    now,
  );
  const refused = { allowed: false as const, retryAfter: secondsLeft(now, MINUTE) };
  if (endpointCount > limits.endpointPerMinute) return refused;
  let active = activeBefore;
  if (endpointCount === 1) {
    [active = activeBefore + 1] = await counters.run(
      [{ counter: minute(`push-device-active:${deviceId}`), increment: true }],
      now,
    );
  }
  const fairShare = Math.floor(limits.devicePerMinute / Math.max(active, 1));
  if (deviceCount >= limits.devicePerMinute && endpointCount > fairShare) return refused;
  return { allowed: true };
}

const breakerCounters = (context: RelayContext, platform: Platform) => {
  const windowSeconds = context.limits.breakerWindowSeconds;
  return {
    deliveries: { bucket: `deliveries:${platform}`, windowSeconds },
    invalidations: { bucket: `invalidations:${platform}`, windowSeconds },
  };
};

/**
 * The invalidation breaker. An unrecognized configuration mistake could make
 * APNs/FCM reject every token; marking them all would make every app delete its
 * subscriptions for good. So when a platform's rejections in the window exceed
 * an absolute count, or a share of its deliveries, the relay stops invalidating
 * on rejections, answers 502 (apps retry later), and logs an alert.
 *
 * It sees only `rejected` answers for devices that were delivered to before.
 * `expired` answers (Unregistered, ExpiredToken, UNREGISTERED) come only for
 * tokens the platform once issued, so they always invalidate; and tokens that
 * never worked are invalidated directly. That way registering junk tokens
 * cannot hold the breaker open and turn real uninstalls into 502s.
 */
async function breakerOpen(
  context: RelayContext,
  platform: Platform,
  now: number,
): Promise<{ open: boolean; invalidations: number; deliveries: number }> {
  const { limits } = context;
  const counters = breakerCounters(context, platform);
  const [invalidations = 0, deliveries = 0] = await context.counters.run(
    [
      { counter: counters.invalidations, increment: true },
      { counter: counters.deliveries, increment: false },
    ],
    now,
  );
  const tooMany = invalidations > limits.breakerMaxInvalidations;
  const tooLarge =
    deliveries >= limits.breakerMinDeliveries && invalidations > limits.breakerMaxRatio * deliveries;
  return { open: tooMany || tooLarge, invalidations, deliveries };
}

const DAY = 24 * 60 * 60;

const gone = () =>
  problem(410, "subscription_gone", "the device behind this subscription is no longer reachable");

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

  if (target.device.invalidatedAt !== null) return gone();

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

  const platform = target.device.platform;
  const admission = await admit(context, target, now);
  if (!admission.allowed) {
    context.log.info("push_rate_limited", { platform });
    return problem(429, "rate_limited", "too many messages for this subscription or device", {
      "retry-after": String(admission.retryAfter),
    });
  }

  const sender = context.senders[platform];
  if (!sender) {
    context.log.error("push_sender_unconfigured", { platform });
    return problem(503, "delivery_unavailable", "delivery to this device's platform is not configured", {
      "retry-after": "3600",
    });
  }

  await context.counters.run(
    [
      { counter: minute(`push-device:${target.device.deviceId}`), increment: true },
      { counter: breakerCounters(context, platform).deliveries, increment: true },
    ],
    now,
  );
  const sentToken = target.device.token;
  const outcome: DeliveryOutcome = await sender.send({
    platform,
    token: sentToken,
    apnsEnvironment: target.device.apnsEnvironment,
    subscriptionId: target.subscriptionId,
    body,
    ttl,
    urgency,
    topic,
    now,
  });

  const fields = { platform, outcome: outcome.kind };
  switch (outcome.kind) {
    case "delivered":
      // At most one write a day: the device is in use, and has been delivered to.
      if (target.device.deliveredAt === null || now - target.device.deliveredAt >= DAY) {
        await context.store.recordDelivery(target.device.deviceId, now);
      }
      context.log.info("push_delivered", { ...fields, bytes: body.length, urgency, ttl });
      return new Response(null, {
        status: 201,
        headers: {
          location: `${origin}/message/${randomId(16)}`,
          ttl: String(ttl),
          "cache-control": "no-store",
        },
      });
    case "device-invalid": {
      const guarded = outcome.evidence === "rejected" && target.device.deliveredAt !== null;
      const breaker = guarded ? await breakerOpen(context, platform, now) : null;
      if (breaker?.open) {
        context.log.error("invalidation_breaker_open", {
          alert: true,
          platform,
          reason: outcome.reason,
          invalidations: breaker.invalidations,
          deliveries: breaker.deliveries,
          windowSeconds: context.limits.breakerWindowSeconds,
        });
        return problem(502, "delivery_failed", "the platform push service rejected too many devices; not invalidating");
      }
      if (await context.store.invalidateDevice(target.device.deviceId, sentToken, now)) {
        context.log.info("push_device_invalid", { ...fields, reason: outcome.reason, evidence: outcome.evidence });
        return gone();
      }
      // Nothing changed: either another push invalidated the same token first,
      // or the device replaced its token while this push was in flight. Only the
      // first is gone; the second must be retried to the new token.
      const current = await context.store.getDevice(target.device.deviceId);
      if (current && current.invalidatedAt !== null && current.token === sentToken) return gone();
      context.log.info("push_token_replaced_in_flight", { platform });
      return problem(503, "delivery_unavailable", "the device replaced its token during delivery; retry", {
        "retry-after": "5",
      });
    }
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
