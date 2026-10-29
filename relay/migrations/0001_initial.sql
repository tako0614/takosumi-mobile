-- Takosumi Mobile push relay: initial schema.
--
-- The relay stores only what it needs to route a push: the device's APNs/FCM
-- token, a hash of the device secret, and per subscription the application
-- server key and the random endpoint id. It never stores an app's origin or a
-- message body.

CREATE TABLE devices (
  device_id TEXT PRIMARY KEY,
  secret_hash TEXT NOT NULL,
  platform TEXT NOT NULL CHECK (platform IN ('apns', 'fcm')),
  token TEXT NOT NULL,
  apns_environment TEXT CHECK (apns_environment IN ('production', 'development')),
  -- Set when APNs/FCM reported the token invalid. Pushes then answer 410 until
  -- the device updates its token; the row is purged after a retention period.
  invalidated_at INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE subscriptions (
  subscription_id TEXT PRIMARY KEY,
  device_id TEXT NOT NULL REFERENCES devices (device_id) ON DELETE CASCADE,
  endpoint_id TEXT NOT NULL UNIQUE,
  application_server_key TEXT NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE INDEX subscriptions_by_device ON subscriptions (device_id);
CREATE INDEX devices_by_invalidated_at ON devices (invalidated_at)
  WHERE invalidated_at IS NOT NULL;

-- Fixed-window counters for rate limits. Buckets are opaque keys (scope plus a
-- device id, endpoint id or a hash of the client address) and are purged after
-- their window has passed.
CREATE TABLE rate_windows (
  bucket TEXT PRIMARY KEY,
  window_start INTEGER NOT NULL,
  count INTEGER NOT NULL
);

CREATE INDEX rate_windows_by_window_start ON rate_windows (window_start);
