// Cloudflare Worker entry for the Takosumi Mobile push relay.

import { createRelay } from "./app.ts";
import { DEFAULT_LIMITS } from "./context.ts";
import type { D1Database } from "./d1.ts";
import { problem } from "./http.ts";
import { consoleLog, type RelayLog } from "./log.ts";
import { D1Counters } from "./rate-limit.ts";
import { ApnsSender } from "./senders/apns.ts";
import { FcmSender, parseServiceAccount } from "./senders/fcm.ts";
import { SinkSender } from "./senders/sink.ts";
import type { PushSender } from "./senders/types.ts";
import { RelayStore, type Platform } from "./store.ts";

export interface RelayEnv {
  DB: D1Database;
  /** `staging` or `production` (required). Only staging may use DELIVERY_MODE=sink. */
  RELAY_ENVIRONMENT?: string;
  /** Public https origin; endpoints are issued under it and VAPID aud must match. Required in production. */
  RELAY_ORIGIN?: string;
  /** `live` (default) delivers through APNs/FCM; `sink` accepts and drops. */
  DELIVERY_MODE?: string;
  APNS_TEAM_ID?: string;
  APNS_KEY_ID?: string;
  APNS_TOPIC?: string;
  /** Secret: the APNs `.p8` key. */
  APNS_PRIVATE_KEY?: string;
  FCM_PROJECT_ID?: string;
  /** Secret: the Firebase service account JSON. */
  FCM_SERVICE_ACCOUNT_JSON?: string;
}

export interface ResolvedConfig {
  origin: string | null;
  senders: Record<Platform, PushSender | null>;
  problems: string[];
}

// Senders hold provider-token caches (APNs JWT, Google OAuth token), so they
// live as long as the isolate and are rebuilt only when their config changes.
const senderCache = new Map<string, PushSender>();

/** FNV-1a: tells a rotated secret apart without keeping it in a map key. */
function fingerprint(text: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < text.length; index += 1) {
    hash ^= text.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash.toString(16);
}

function cached(key: string, build: () => PushSender): PushSender {
  let sender = senderCache.get(key);
  if (!sender) {
    sender = build();
    senderCache.set(key, sender);
  }
  return sender;
}

export function resolveConfig(env: RelayEnv): ResolvedConfig {
  const problems: string[] = [];
  if (env.RELAY_ENVIRONMENT !== "staging" && env.RELAY_ENVIRONMENT !== "production") {
    problems.push("RELAY_ENVIRONMENT must be staging or production");
  }
  if (env.RELAY_ENVIRONMENT === "production" && !env.RELAY_ORIGIN) {
    // Without it, endpoints and the VAPID audience would follow whatever host a
    // request arrived on.
    problems.push("RELAY_ORIGIN is required in production");
  }
  let origin: string | null = null;
  if (env.RELAY_ORIGIN) {
    try {
      const parsed = new URL(env.RELAY_ORIGIN);
      if (parsed.protocol !== "https:" || parsed.origin !== env.RELAY_ORIGIN.replace(/\/$/u, "")) {
        problems.push("RELAY_ORIGIN must be a bare https origin");
      } else {
        origin = parsed.origin;
      }
    } catch {
      problems.push("RELAY_ORIGIN is not a URL");
    }
  }

  const mode = env.DELIVERY_MODE ?? "live";
  if (mode === "sink") {
    if (env.RELAY_ENVIRONMENT !== "staging") {
      problems.push("DELIVERY_MODE=sink is allowed only when RELAY_ENVIRONMENT=staging");
    }
    const sink = new SinkSender();
    return { origin, senders: { apns: sink, fcm: sink }, problems };
  }
  if (mode !== "live") problems.push("DELIVERY_MODE is live or sink");

  let apns: PushSender | null = null;
  if (env.APNS_TEAM_ID && env.APNS_KEY_ID && env.APNS_TOPIC && env.APNS_PRIVATE_KEY) {
    const config = {
      teamId: env.APNS_TEAM_ID,
      keyId: env.APNS_KEY_ID,
      topic: env.APNS_TOPIC,
      privateKeyPem: env.APNS_PRIVATE_KEY,
    };
    apns = cached(`apns:${config.teamId}:${config.keyId}:${config.topic}:${fingerprint(config.privateKeyPem)}`, () =>
      new ApnsSender(config),
    );
  }

  let fcm: PushSender | null = null;
  if (env.FCM_PROJECT_ID && env.FCM_SERVICE_ACCOUNT_JSON) {
    const serviceAccount = parseServiceAccount(env.FCM_SERVICE_ACCOUNT_JSON);
    if (!serviceAccount) {
      problems.push("FCM_SERVICE_ACCOUNT_JSON is not a service account key");
    } else {
      const projectId = env.FCM_PROJECT_ID;
      fcm = cached(`fcm:${projectId}:${serviceAccount.client_email}:${fingerprint(serviceAccount.private_key)}`, () =>
        new FcmSender({ projectId, serviceAccount }),
      );
    }
  }
  return { origin, senders: { apns, fcm }, problems };
}

export function relayForEnv(env: RelayEnv, log: RelayLog = consoleLog) {
  const config = resolveConfig(env);
  if (config.problems.length > 0) return { config, relay: null };
  return {
    config,
    relay: createRelay({
      store: new RelayStore(env.DB),
      counters: new D1Counters(env.DB),
      senders: config.senders,
      origin: config.origin,
      limits: DEFAULT_LIMITS,
      log,
      now: () => Math.floor(Date.now() / 1000),
    }),
  };
}

export default {
  async fetch(request: Request, env: RelayEnv): Promise<Response> {
    const { config, relay } = relayForEnv(env);
    if (!relay) {
      consoleLog.error("relay_misconfigured", { problems: config.problems.join("; ") });
      return problem(503, "relay_misconfigured", "the relay is not configured correctly");
    }
    return relay.fetch(request);
  },
  async scheduled(_controller: unknown, env: RelayEnv): Promise<void> {
    const { config, relay } = relayForEnv(env);
    if (!relay) {
      consoleLog.error("relay_misconfigured", { problems: config.problems.join("; ") });
      return;
    }
    await relay.scheduled();
  },
};
