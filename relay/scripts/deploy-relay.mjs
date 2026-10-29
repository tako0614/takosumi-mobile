// Deploy surfaces of the Takosumi Mobile push relay (Cloudflare Worker + D1).
//
// The repository's single entrypoint is `scripts/deploy.mjs`; it imports the
// declarations below for `--contract` and calls the runners only for the
// surface that was asked for. The common obligations are takos-control's
// `engineering.policy.json` → `deploy`.
//
// Four surfaces, one failure mode each:
//
//   push-relay-staging         Worker code + generated config, disposable staging target
//   push-relay                 Worker code only, production (vars, secrets, crons, domain untouched)
//   push-relay-staging-schema  D1 migrations, staging database
//   push-relay-schema          D1 migrations, production database
//
// Realized identifiers (database ids, the public origin) come from the
// operator's environment and are written only to a temporary config outside the
// repository. APNs/FCM secrets are never handled here: they are set with
// `wrangler secret put` as a separate credential operation (see relay/README.md).

import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { lineageVerdict } from "./lineage.mjs";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const relayRoot = join(repo, "relay");
const COMPATIBILITY_DATE = "2026-06-01";
const PURGE_CRON = "17 3 * * *";
const GATE = ["bun", "run", "relay:check"];

export const RELAY_SURFACES = {
  staging: {
    surface: "push-relay-staging",
    worker: "takosumi-push-relay-staging",
    database: "takosumi-push-relay-staging",
    environment: "staging",
    lineage: "integration",
  },
  production: {
    surface: "push-relay",
    worker: "takosumi-push-relay",
    database: "takosumi-push-relay",
    environment: "production",
    lineage: "production-routine",
  },
  stagingSchema: {
    surface: "push-relay-staging-schema",
    database: "takosumi-push-relay-staging",
    environment: "staging",
    lineage: "production-routine",
  },
  productionSchema: {
    surface: "push-relay-schema",
    database: "takosumi-push-relay",
    environment: "production",
    lineage: "production-routine",
  },
};

const S = RELAY_SURFACES;

export const RELAY_CONTRACT_SURFACES = [
  {
    surface: S.staging.surface,
    target: `cloudflare-worker:${S.staging.worker}`,
    triggers: [],
    lineage: S.staging.lineage,
    requiresScripts: ["relay:check", "relay:smoke"],
    requiresTools: ["git", "bun", "wrangler"],
    requiresEnv: [
      "PUSH_RELAY_STAGING_D1_DATABASE_ID",
      "PUSH_RELAY_STAGING_DELIVERY_MODE",
      "PUSH_RELAY_STAGING_ORIGIN",
      "PUSH_RELAY_APNS_TEAM_ID",
      "PUSH_RELAY_APNS_KEY_ID",
      "PUSH_RELAY_APNS_TOPIC",
      "PUSH_RELAY_FCM_PROJECT_ID",
    ],
    obligations: {
      provenance:
        "integration lineage: accepts the exact worktree, dirty or not, and records its commit and dirty flag. Runs `bun run relay:check` once (relay typecheck and every relay test), bundles relay/src/index.ts with Bun.build once, records the bundle sha256, and uploads exactly that file with `wrangler deploy --no-bundle`. The Wrangler config is generated in a temporary directory from PUSH_RELAY_STAGING_D1_DATABASE_ID (required), PUSH_RELAY_STAGING_DELIVERY_MODE (`sink` by default, or `live`), the optional PUSH_RELAY_STAGING_ORIGIN, and, for live delivery, PUSH_RELAY_APNS_TEAM_ID, PUSH_RELAY_APNS_KEY_ID, PUSH_RELAY_APNS_TOPIC and PUSH_RELAY_FCM_PROJECT_ID; the staging Worker's vars, workers.dev route and purge cron are wholly generated on every deploy because the target is disposable",
      "post-conditions":
        "reads `wrangler deployments status` back and requires the uploaded version at 100 percent, then runs `bun run relay:smoke` against the deployed origin: health, device registration, subscription, VAPID 401/403, unknown endpoint 404, an RFC 8291 encrypted Web Push answered 201 in sink mode (410 per configured platform in live mode, which proves APNs/FCM accepted our credentials and rejected the fake token), and device deletion",
      reversal:
        "the version serving before the upload is read and printed first (none on the first deploy); restore it with `wrangler rollback <version-id> --name takosumi-push-relay-staging`, or delete the disposable staging Worker with `wrangler delete`",
      "failure-handling":
        "prints Wrangler's own stdout and stderr; reports PRE_UPLOAD_FAILURE (target untouched) for lineage, environment, gate, build or config errors and POST_UPLOAD_INDETERMINATE after the upload starts, naming the previous version; never retries an upload",
    },
  },
  {
    surface: S.production.surface,
    target: `cloudflare-worker:${S.production.worker}`,
    triggers: [],
    lineage: S.production.lineage,
    requiresScripts: ["relay:check", "relay:smoke"],
    requiresTools: ["git", "bun", "wrangler"],
    requiresEnv: ["PUSH_RELAY_D1_DATABASE_ID", "PUSH_RELAY_ORIGIN"],
    obligations: {
      provenance:
        "production-routine lineage: refuses a dirty worktree, a branch other than main, or a HEAD that is not origin/main or an ancestor of it after a fresh fetch. Runs `bun run relay:check` once, bundles relay/src/index.ts with Bun.build once, records the commit and bundle sha256, and uploads exactly that file with `wrangler versions upload --no-bundle --keep-vars`. The generated config carries only the Worker name, the bundle, the compatibility date and the D1 binding to PUSH_RELAY_D1_DATABASE_ID; vars, secrets, the custom domain for PUSH_RELAY_ORIGIN and the purge cron belong to provisioning and are left as they are",
      "post-conditions":
        "deploys the new version at 100 percent, reads `wrangler deployments status` back and requires exactly that version, then runs `bun run relay:smoke` against PUSH_RELAY_ORIGIN with live expectations: a push to a fake token of each configured platform must come back 410 from APNs/FCM; the smoke deletes the device it registered",
      reversal:
        "refuses to upload unless a current production version is readable, prints it, and restores with `wrangler rollback <previous-version-id> --name takosumi-push-relay`; code versions do not change D1, so rollback is safe while the schema stays additive",
      "failure-handling":
        "prints Wrangler's own output; PRE_UPLOAD_FAILURE before the upload (target untouched); POST_UPLOAD_INDETERMINATE after it, naming the previous version and telling the operator to read `wrangler deployments status` before doing anything; never retries an upload or a version deploy",
    },
  },
  {
    surface: S.stagingSchema.surface,
    target: `cloudflare-d1:${S.stagingSchema.database}`,
    triggers: ["irreversible"],
    lineage: S.stagingSchema.lineage,
    requiresScripts: ["relay:check"],
    requiresTools: ["git", "bun", "wrangler"],
    requiresEnv: ["PUSH_RELAY_STAGING_D1_DATABASE_ID"],
    obligations: {
      provenance:
        "production-routine lineage (the SQL must be merged to main before it touches any shared database); applies relay/migrations from that commit to PUSH_RELAY_STAGING_D1_DATABASE_ID with `wrangler d1 migrations apply --remote` and prints the commit",
      "pre-mutation-proof":
        "`bun run relay:check` applies every migration to SQLite and runs the relay against it; the same migrations are then applied to a fresh local D1 (`wrangler d1 migrations apply --local --persist-to <temporary directory>`); the pending list on the target is printed before applying",
      "independent-review":
        "requires `--reviewed-by <name>` naming who reviewed the migration SQL in its pull request without having written it (a person or a non-authoring agent), and prints it",
      "post-conditions":
        "`wrangler d1 migrations list --remote` must report nothing pending, and the expected tables must exist",
      reversal:
        "migrations are forward-only; the staging database holds only disposable test devices, so it is repaired forward or recreated from relay/migrations (`wrangler d1 delete` and a fresh apply)",
      "failure-handling":
        "prints Wrangler's own output; any failure before `migrations apply --remote` leaves the target untouched; a failure during it is indeterminate and is resolved by reading `migrations list --remote`, never by re-running blindly",
    },
  },
  {
    surface: S.productionSchema.surface,
    target: `cloudflare-d1:${S.productionSchema.database}`,
    triggers: ["irreversible"],
    lineage: S.productionSchema.lineage,
    requiresScripts: ["relay:check"],
    requiresTools: ["git", "bun", "wrangler"],
    requiresEnv: ["PUSH_RELAY_D1_DATABASE_ID", "PUSH_RELAY_STAGING_D1_DATABASE_ID", "PUSH_RELAY_BACKUP_DIR"],
    obligations: {
      provenance:
        "production-routine lineage; applies relay/migrations from that commit to PUSH_RELAY_D1_DATABASE_ID with `wrangler d1 migrations apply --remote` and prints the commit",
      "pre-mutation-proof":
        "`bun run relay:check` and a fresh local D1 apply as for staging, and the staging database PUSH_RELAY_STAGING_D1_DATABASE_ID must already report nothing pending for the same migrations (rehearsed through push-relay-staging-schema); a full `wrangler d1 export --remote` is written to PUSH_RELAY_BACKUP_DIR (outside the repository) before applying",
      "independent-review":
        "requires `--reviewed-by <name>` naming who reviewed the migration SQL without having written it (a person or a non-authoring agent), and prints it",
      "post-conditions":
        "`wrangler d1 migrations list --remote` must report nothing pending and the expected tables must exist; then run `bun run deploy -- push-relay`, whose smoke exercises every table",
      reversal:
        "forward-only: no down-migrations. Repair forward with a new migration; the export in PUSH_RELAY_BACKUP_DIR is the last resort for data, restored only as a separate deliberate operation",
      "failure-handling":
        "prints Wrangler's own output; failures before the remote apply leave production untouched; a failure during it is indeterminate and is resolved by reading `migrations list --remote` and the export, never by re-running blindly",
    },
  },
];

// ---------------------------------------------------------------------------

function die(message, detail = []) {
  process.stderr.write(`deploy blocked: ${message}\n`);
  for (const line of detail) process.stderr.write(`- ${line}\n`);
  process.exit(1);
}

function phaseFailure(phase, message, detail = []) {
  process.stderr.write(`\n${phase}: ${message}\n`);
  for (const line of detail) process.stderr.write(`- ${line}\n`);
  process.exit(1);
}

/** Runs a command, echoing its output; returns { status, stdout, stderr }. */
function run(command, args, options = {}) {
  process.stdout.write(`\n$ ${[command, ...args].join(" ")}\n`);
  const result = spawnSync(command, args, {
    cwd: options.cwd ?? repo,
    encoding: "utf8",
    env: options.env ?? process.env,
    maxBuffer: 64 * 1024 * 1024,
  });
  if (result.stdout) process.stdout.write(result.stdout);
  if (result.stderr) process.stderr.write(result.stderr);
  return { status: result.status ?? 1, stdout: result.stdout ?? "", stderr: result.stderr ?? "" };
}

function git(...args) {
  const result = spawnSync("git", args, { cwd: repo, encoding: "utf8" });
  return (result.stdout ?? "").trim();
}

function requireLineage(lineageClass) {
  const verdict = lineageVerdict(lineageClass, repo);
  if (verdict.verdict !== "accept") die(`${lineageClass} lineage refuses this source: ${verdict.why}`);
  process.stdout.write(`lineage ${lineageClass}: ${verdict.why}\n`);
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u;

function requireDatabaseId(name, value) {
  if (!value) die(`${name} is not set`, ["create the database with `wrangler d1 create` (provisioning) and export its id"]);
  if (!UUID.test(value)) die(`${name} is not a D1 database id`);
  return value;
}

function optionValue(args, name) {
  const index = args.indexOf(name);
  if (index === -1) return null;
  const value = args[index + 1];
  return value && !value.startsWith("--") ? value : null;
}

/** The gate runs without agent/CI variables that change test output or behavior. */
function gateEnv() {
  const env = { ...process.env };
  for (const name of ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "AI_AGENT", "NODE_ENV"]) delete env[name];
  return env;
}

function runGate() {
  const gate = run(GATE[0], GATE.slice(1), { env: gateEnv() });
  if (gate.status !== 0) phaseFailure("PRE_UPLOAD_FAILURE", `\`${GATE.join(" ")}\` failed; the target was not touched`);
}

async function buildBundle(outDir) {
  const result = await Bun.build({
    entrypoints: [join(relayRoot, "src", "index.ts")],
    target: "browser",
    format: "esm",
    minify: false,
  });
  if (!result.success || result.outputs.length !== 1) {
    phaseFailure("PRE_UPLOAD_FAILURE", "the Worker bundle did not build", result.logs.map(String));
  }
  const bytes = new Uint8Array(await result.outputs[0].arrayBuffer());
  const path = join(outDir, "worker.js");
  writeFileSync(path, bytes);
  return { path, sha256: createHash("sha256").update(bytes).digest("hex"), bytes: bytes.length };
}

function wranglerConfig(outDir, config) {
  const path = join(outDir, "wrangler.json");
  writeFileSync(path, `${JSON.stringify(config, null, 2)}\n`);
  return path;
}

function d1Binding(database, databaseId) {
  return {
    binding: "DB",
    database_name: database,
    database_id: databaseId,
    migrations_dir: join(relayRoot, "migrations"),
  };
}

/** The version currently serving 100 percent, or null when none is readable. */
function currentVersion(worker) {
  const result = spawnSync("wrangler", ["deployments", "status", "--name", worker, "--json"], {
    cwd: repo,
    encoding: "utf8",
  });
  if (result.status !== 0) return null;
  try {
    const parsed = JSON.parse(result.stdout);
    const versions = parsed.versions ?? [];
    const full = versions.find((entry) => entry.percentage === 100) ?? versions[0];
    return full?.version_id ?? null;
  } catch {
    return null;
  }
}

function smoke(origin, expectation, platforms) {
  const args = ["run", "relay:smoke", "--", "--origin", origin, "--expect", expectation];
  for (const platform of platforms) args.push("--platform", platform);
  return run("bun", args, { env: gateEnv() }).status === 0;
}

export async function runWorkerSurface(kind) {
  const target = kind === "staging" ? S.staging : S.production;
  requireLineage(target.lineage);

  const commit = git("rev-parse", "HEAD");
  const dirty = git("status", "--porcelain=v1", "--untracked-files=no").length > 0;
  process.stdout.write(`source ${commit}${dirty ? " (dirty worktree)" : ""}\n`);

  let config;
  let origin = null;
  let expectation = "sink";
  let platforms = ["fcm"];
  const outDir = mkdtempSync(join(tmpdir(), "push-relay-deploy-"));
  try {
    if (kind === "staging") {
      const databaseId = requireDatabaseId("PUSH_RELAY_STAGING_D1_DATABASE_ID", process.env.PUSH_RELAY_STAGING_D1_DATABASE_ID);
      const mode = process.env.PUSH_RELAY_STAGING_DELIVERY_MODE ?? "sink";
      if (mode !== "sink" && mode !== "live") die("PUSH_RELAY_STAGING_DELIVERY_MODE is sink or live");
      const vars = { RELAY_ENVIRONMENT: "staging", DELIVERY_MODE: mode };
      origin = process.env.PUSH_RELAY_STAGING_ORIGIN ?? null;
      if (origin) vars.RELAY_ORIGIN = origin;
      if (mode === "live") {
        expectation = "live";
        platforms = [];
        const apns = {
          APNS_TEAM_ID: process.env.PUSH_RELAY_APNS_TEAM_ID,
          APNS_KEY_ID: process.env.PUSH_RELAY_APNS_KEY_ID,
          APNS_TOPIC: process.env.PUSH_RELAY_APNS_TOPIC,
        };
        if (Object.values(apns).every(Boolean)) {
          Object.assign(vars, apns);
          platforms.push("apns");
        }
        if (process.env.PUSH_RELAY_FCM_PROJECT_ID) {
          vars.FCM_PROJECT_ID = process.env.PUSH_RELAY_FCM_PROJECT_ID;
          platforms.push("fcm");
        }
        if (platforms.length === 0) die("live delivery needs the APNs or FCM identifiers");
      }
      config = {
        name: target.worker,
        main: "worker.js",
        compatibility_date: COMPATIBILITY_DATE,
        workers_dev: true,
        d1_databases: [d1Binding(target.database, databaseId)],
        triggers: { crons: [PURGE_CRON] },
        vars,
        observability: { enabled: true },
      };
    } else {
      const databaseId = requireDatabaseId("PUSH_RELAY_D1_DATABASE_ID", process.env.PUSH_RELAY_D1_DATABASE_ID);
      origin = process.env.PUSH_RELAY_ORIGIN ?? null;
      if (!origin || !/^https:\/\/[a-z0-9.-]+$/u.test(origin)) die("PUSH_RELAY_ORIGIN must be the relay's https origin");
      expectation = "live";
      platforms = [];
      config = {
        name: target.worker,
        main: "worker.js",
        compatibility_date: COMPATIBILITY_DATE,
        workers_dev: false,
        d1_databases: [d1Binding(target.database, databaseId)],
      };
    }

    runGate();
    const bundle = await buildBundle(outDir);
    process.stdout.write(`\nbundle ${bundle.bytes} bytes, sha256 ${bundle.sha256}\n`);
    const configPath = wranglerConfig(outDir, config);

    const previous = currentVersion(target.worker);
    if (kind === "production" && !previous) {
      die("no current production version is readable, so there is no rollback point", [
        "provision the Worker, its custom domain, vars, secrets and cron first (relay/README.md)",
      ]);
    }
    process.stdout.write(`previous version ${previous ?? "none (first deploy)"}\n`);

    let version;
    let deployedUrl = null;
    const message = `${commit.slice(0, 12)}${dirty ? "-dirty" : ""} sha256:${bundle.sha256.slice(0, 16)}`;
    if (kind === "staging") {
      const upload = run("wrangler", ["deploy", "--config", configPath, "--no-bundle", "--message", message]);
      if (upload.status !== 0) {
        phaseFailure("POST_UPLOAD_INDETERMINATE", "wrangler deploy failed after it may have reached Cloudflare", [
          `read \`wrangler deployments status --name ${target.worker}\` before retrying`,
          `previous version ${previous ?? "none"}`,
        ]);
      }
      version = upload.stdout.match(/Current Version ID:\s*([0-9a-f-]{36})/u)?.[1] ?? null;
      deployedUrl = upload.stdout.match(/https:\/\/[a-z0-9.-]+\.workers\.dev/u)?.[0] ?? null;
    } else {
      const upload = run("wrangler", [
        "versions",
        "upload",
        "--config",
        configPath,
        "--no-bundle",
        "--keep-vars",
        "--message",
        message,
        "--tag",
        commit.slice(0, 12),
      ]);
      version = upload.stdout.match(/Worker Version ID:\s*([0-9a-f-]{36})/u)?.[1] ?? null;
      if (upload.status !== 0 || !version) {
        phaseFailure("POST_UPLOAD_INDETERMINATE", "the version upload did not report a version id", [
          `production still serves ${previous}; read \`wrangler versions list --name ${target.worker}\``,
        ]);
      }
      const activate = run("wrangler", [
        "versions",
        "deploy",
        `${version}@100%`,
        "--name",
        target.worker,
        "--message",
        message,
        "--yes",
      ]);
      if (activate.status !== 0) {
        phaseFailure("POST_UPLOAD_INDETERMINATE", "the version deploy failed", [
          `read \`wrangler deployments status --name ${target.worker}\`; previous version ${previous}`,
        ]);
      }
    }

    const serving = currentVersion(target.worker);
    const readback = version !== null && serving === version;
    const smokeOrigin = origin ?? deployedUrl;
    const smoked = smokeOrigin ? smoke(smokeOrigin, expectation, platforms) : false;
    const result = {
      kind: "takos.deploy-result@v1",
      surface: target.surface,
      target: `cloudflare-worker:${target.worker}`,
      commit,
      dirty,
      bundleSha256: bundle.sha256,
      version,
      previousVersion: previous,
      origin: smokeOrigin,
      readback: readback ? "SERVING_UPLOADED_VERSION" : `MISMATCH (serving ${serving ?? "unknown"})`,
      smoke: smokeOrigin ? (smoked ? "PASSED" : "FAILED") : "NO_ORIGIN",
      status: readback && smoked ? "DEPLOYED" : "INDETERMINATE",
    };
    process.stdout.write(`\n${JSON.stringify(result, null, 2)}\n`);
    if (result.status !== "DEPLOYED") {
      phaseFailure("POST_UPLOAD_INDETERMINATE", "the post-conditions did not hold", [
        `previous version ${previous ?? "none"}; roll back with \`wrangler rollback <id> --name ${target.worker}\` after reading the deployment status`,
      ]);
    }
  } finally {
    rmSync(outDir, { recursive: true, force: true });
  }
}

const EXPECTED_TABLES = ["devices", "rate_windows", "subscriptions"];

function nothingPending(output) {
  return /No migrations to apply/iu.test(output);
}

function schemaConfig(outDir, database, databaseId) {
  return wranglerConfig(outDir, {
    name: "takosumi-push-relay-schema",
    compatibility_date: COMPATIBILITY_DATE,
    d1_databases: [d1Binding(database, databaseId)],
  });
}

export async function runSchemaSurface(kind, args) {
  const target = kind === "staging" ? S.stagingSchema : S.productionSchema;
  requireLineage(target.lineage);
  const reviewer = optionValue(args, "--reviewed-by");
  if (!reviewer) die("--reviewed-by <name> is required: who reviewed the migration SQL without writing it");
  const commit = git("rev-parse", "HEAD");
  process.stdout.write(`source ${commit}\nreviewed by ${reviewer}\n`);

  const databaseId =
    kind === "staging"
      ? requireDatabaseId("PUSH_RELAY_STAGING_D1_DATABASE_ID", process.env.PUSH_RELAY_STAGING_D1_DATABASE_ID)
      : requireDatabaseId("PUSH_RELAY_D1_DATABASE_ID", process.env.PUSH_RELAY_D1_DATABASE_ID);
  let backupDir = null;
  let stagingId = null;
  if (kind === "production") {
    stagingId = requireDatabaseId("PUSH_RELAY_STAGING_D1_DATABASE_ID", process.env.PUSH_RELAY_STAGING_D1_DATABASE_ID);
    backupDir = process.env.PUSH_RELAY_BACKUP_DIR ?? null;
    if (!backupDir || !isAbsolute(backupDir)) die("PUSH_RELAY_BACKUP_DIR must be an absolute directory outside the repository");
    if (resolve(backupDir).startsWith(repo)) die("PUSH_RELAY_BACKUP_DIR must be outside the repository");
  }

  const outDir = mkdtempSync(join(tmpdir(), "push-relay-schema-"));
  try {
    runGate();
    const configPath = schemaConfig(outDir, target.database, databaseId);
    const local = run("wrangler", [
      "d1", "migrations", "apply", "DB", "--local", "--persist-to", join(outDir, "local-d1"), "--config", configPath,
    ]);
    if (local.status !== 0) phaseFailure("PRE_UPLOAD_FAILURE", "the migrations do not apply to a fresh local D1");

    if (kind === "production") {
      const stagingDir = join(outDir, "staging");
      mkdirSync(stagingDir);
      const stagingConfig = schemaConfig(stagingDir, S.stagingSchema.database, stagingId);
      const rehearsed = run("wrangler", ["d1", "migrations", "list", "DB", "--remote", "--config", stagingConfig]);
      if (rehearsed.status !== 0 || !nothingPending(rehearsed.stdout)) {
        die("staging has pending migrations; apply them with push-relay-staging-schema first");
      }
    }

    const pending = run("wrangler", ["d1", "migrations", "list", "DB", "--remote", "--config", configPath]);
    if (pending.status !== 0) phaseFailure("PRE_UPLOAD_FAILURE", "cannot read the target's migration state");
    if (nothingPending(pending.stdout)) {
      process.stdout.write("\nnothing to apply\n");
      return;
    }

    if (backupDir) {
      mkdirSync(backupDir, { recursive: true });
      const file = join(backupDir, `${target.database}-${new Date().toISOString().replace(/[:.]/gu, "-")}.sql`);
      const exported = run("wrangler", ["d1", "export", "DB", "--remote", "--output", file, "--config", configPath]);
      if (exported.status !== 0 || !existsSync(file)) phaseFailure("PRE_UPLOAD_FAILURE", "the backup export failed");
      process.stdout.write(`backup ${file} sha256 ${createHash("sha256").update(readFileSync(file)).digest("hex")}\n`);
    }

    const applied = run("wrangler", ["d1", "migrations", "apply", "DB", "--remote", "--config", configPath], {
      env: { ...process.env, CI: "true" },
    });
    const after = run("wrangler", ["d1", "migrations", "list", "DB", "--remote", "--config", configPath]);
    const tables = run("wrangler", [
      "d1", "execute", "DB", "--remote", "--json", "--config", configPath,
      "--command", "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name",
    ]);
    const present = EXPECTED_TABLES.every((name) => tables.stdout.includes(`"${name}"`));
    const result = {
      kind: "takos.deploy-result@v1",
      surface: target.surface,
      target: `cloudflare-d1:${target.database}`,
      commit,
      reviewedBy: reviewer,
      applied: applied.status === 0,
      nothingPending: after.status === 0 && nothingPending(after.stdout),
      tablesPresent: present,
      status: applied.status === 0 && nothingPending(after.stdout) && present ? "APPLIED" : "INDETERMINATE",
    };
    process.stdout.write(`\n${JSON.stringify(result, null, 2)}\n`);
    if (result.status !== "APPLIED") {
      phaseFailure("POST_UPLOAD_INDETERMINATE", "the migration state could not be confirmed", [
        "read `wrangler d1 migrations list --remote` before doing anything else; do not re-run blindly",
      ]);
    }
  } finally {
    rmSync(outDir, { recursive: true, force: true });
  }
}
