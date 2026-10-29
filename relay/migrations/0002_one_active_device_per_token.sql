-- At most one active device per (platform, token), and device activity.
--
-- A registration whose token another active device already holds supersedes
-- that device: it is marked invalid, its endpoints answer 410, and the apps
-- behind them subscribe again. The partial unique index makes the invariant
-- hold even under concurrent registrations.
--
-- seen_at is the last authenticated call from the device (registration, token
-- update, subscription changes). Devices unseen for a long period are deleted
-- by the scheduled cleanup.

ALTER TABLE devices ADD COLUMN seen_at INTEGER NOT NULL DEFAULT 0;
UPDATE devices SET seen_at = updated_at;

-- Rows that already share an active token: keep the newest, supersede the rest.
UPDATE devices SET invalidated_at = updated_at
WHERE invalidated_at IS NULL
  AND EXISTS (
    SELECT 1 FROM devices AS newer
    WHERE newer.platform = devices.platform
      AND newer.token = devices.token
      AND newer.invalidated_at IS NULL
      AND (newer.created_at > devices.created_at
        OR (newer.created_at = devices.created_at AND newer.device_id > devices.device_id))
  );

CREATE UNIQUE INDEX devices_one_active_token ON devices (platform, token)
  WHERE invalidated_at IS NULL;
CREATE INDEX devices_by_token ON devices (platform, token);
CREATE INDEX devices_by_seen_at ON devices (seen_at);
