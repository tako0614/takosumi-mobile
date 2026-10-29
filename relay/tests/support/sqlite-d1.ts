// A D1 binding backed by bun:sqlite, with the relay's real migrations applied.
// D1 is SQLite with foreign keys enforced, so the relay's SQL runs unchanged.

import { Database } from "bun:sqlite";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import type { D1Database, D1PreparedStatement, D1Result } from "../../src/d1.ts";

const MIGRATIONS = join(import.meta.dir, "..", "..", "migrations");

type Binding = string | number | bigint | boolean | null | Uint8Array;

class SqliteStatement implements D1PreparedStatement {
  constructor(
    private readonly sqlite: Database,
    private readonly sql: string,
    private readonly params: Binding[] = [],
  ) {}

  bind(...values: unknown[]): D1PreparedStatement {
    for (const value of values) {
      if (value === undefined) throw new TypeError("D1 does not accept undefined bindings");
    }
    return new SqliteStatement(this.sqlite, this.sql, values as Binding[]);
  }

  rows<T>(): T[] {
    return this.sqlite.query(this.sql).all(...this.params) as T[];
  }

  async first<T>(): Promise<T | null> {
    return (this.sqlite.query(this.sql).get(...this.params) as T | null) ?? null;
  }

  async all<T>(): Promise<D1Result<T>> {
    return { results: this.rows<T>(), success: true };
  }

  async run(): Promise<D1Result> {
    const result = this.sqlite.query(this.sql).run(...this.params);
    return { results: [], success: true, meta: { changes: result.changes } };
  }
}

export function createSqliteD1(): { db: D1Database; sqlite: Database } {
  const sqlite = new Database(":memory:");
  sqlite.exec("PRAGMA foreign_keys = ON");
  for (const file of readdirSync(MIGRATIONS).filter((name) => name.endsWith(".sql")).sort()) {
    sqlite.exec(readFileSync(join(MIGRATIONS, file), "utf8"));
  }
  const db: D1Database = {
    prepare: (sql) => new SqliteStatement(sqlite, sql),
    async batch<T>(statements: D1PreparedStatement[]): Promise<D1Result<T>[]> {
      return sqlite.transaction(() =>
        statements.map((statement) => ({
          results: (statement as SqliteStatement).rows<T>(),
          success: true,
        })),
      )();
    },
  };
  return { db, sqlite };
}
