import type { DeliveryOutcome, PushSender } from "./types.ts";

/**
 * Accepts every message and delivers none. Only for staging, where the relay's
 * own behavior (VAPID, limits, storage, 201) is exercised before APNs keys and
 * a Firebase project exist. The Worker refuses this mode in production.
 */
export class SinkSender implements PushSender {
  async send(): Promise<DeliveryOutcome> {
    return { kind: "delivered" };
  }
}
