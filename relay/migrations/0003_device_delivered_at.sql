-- When a push to the device last succeeded, written at most once a day.
--
-- A device that keeps receiving pushes is in use, so a successful delivery
-- also refreshes seen_at (the inactivity cleanup reads it). And the
-- invalidation breaker counts only rejections of devices that were delivered
-- to before: tokens that never worked cannot hold the breaker open.

ALTER TABLE devices ADD COLUMN delivered_at INTEGER;
