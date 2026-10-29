import { expect, test } from "bun:test";
import { Database } from "bun:sqlite";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const migration = (name: string) => readFileSync(join(import.meta.dir, "..", "migrations", name), "utf8");

test("0002 keeps the newest active device per token, supersedes the rest, and backfills seen_at", () => {
  const sqlite = new Database(":memory:");
  sqlite.exec("PRAGMA foreign_keys = ON");
  sqlite.exec(migration("0001_initial.sql"));
  const insert = sqlite.query(
    "INSERT INTO devices (device_id, secret_hash, platform, token, apns_environment, invalidated_at, created_at, updated_at) VALUES (?1, 'h', ?2, ?3, NULL, ?4, ?5, ?5)",
  );
  insert.run("old", "fcm", "shared", null, 100);
  insert.run("new", "fcm", "shared", null, 200);
  insert.run("gone", "fcm", "shared", 50, 10);
  insert.run("apns-same-text", "apns", "shared", null, 150);
  insert.run("alone", "fcm", "other", null, 300);

  sqlite.exec(migration("0002_one_active_device_per_token.sql"));

  const rows = sqlite.query("SELECT device_id, invalidated_at, seen_at FROM devices ORDER BY device_id").all();
  expect(rows).toEqual([
    { device_id: "alone", invalidated_at: null, seen_at: 300 },
    { device_id: "apns-same-text", invalidated_at: null, seen_at: 150 },
    { device_id: "gone", invalidated_at: 50, seen_at: 10 },
    { device_id: "new", invalidated_at: null, seen_at: 200 },
    { device_id: "old", invalidated_at: 100, seen_at: 100 },
  ]);
  expect(() => insert.run("again", "fcm", "shared", null, 400)).toThrow();
});
