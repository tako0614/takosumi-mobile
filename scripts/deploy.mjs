#!/usr/bin/env bun

// takosumi-mobile's only deploy entrypoint.
//
// The common obligations and triggers are takos-control's
// `engineering.policy.json` → `deploy`. The surfaces today are the push relay's
// (relay/README.md); app-store delivery of the mobile app is not a surface here.
//
//   bun run deploy -- push-relay-staging
//   bun run deploy -- push-relay [--platform apns] [--platform fcm]
//   bun run deploy -- push-relay-staging-schema --reviewed-by <name>
//   bun run deploy -- push-relay-schema --reviewed-by <name>
//
// `--contract` prints the surfaces with their triggers and how each obligation
// is met, with no side effects. `--lineage-selftest <corpusDir> <class>` runs
// the lineage function that guards the real deploy over control's corpus.

import process from "node:process";

import { RELAY_CONTRACT_SURFACES, RELAY_SURFACES } from "../relay/scripts/deploy-relay.mjs";
import { LINEAGE_CLASSES, runLineageSelfTest } from "../relay/scripts/lineage.mjs";

const CONTRACT = {
  kind: "takos.deploy-contract@v3",
  surfaces: RELAY_CONTRACT_SURFACES,
};

if (process.argv.includes("--contract")) {
  process.stdout.write(`${JSON.stringify(CONTRACT, null, 2)}\n`);
  process.exit(0);
}

if (process.argv[2] === "--lineage-selftest") {
  const corpusRoot = process.argv[3];
  const lineageClass = process.argv[4];
  if (!corpusRoot || !LINEAGE_CLASSES.includes(lineageClass)) {
    process.stderr.write("usage: bun run deploy -- --lineage-selftest <corpusDir> <lineage-class>\n");
    process.exit(1);
  }
  process.stdout.write(`${JSON.stringify(runLineageSelfTest(corpusRoot, lineageClass), null, 2)}\n`);
  process.exit(0);
}

const requested = process.argv[2];
const known = CONTRACT.surfaces.map((entry) => entry.surface);
if (!requested || !known.includes(requested)) {
  process.stderr.write(`usage: bun run deploy -- <surface>\nknown surfaces: ${known.join(", ")}\n`);
  process.exit(1);
}

const { runSchemaSurface, runWorkerSurface } = await import("../relay/scripts/deploy-relay.mjs");
const args = process.argv.slice(3);
switch (requested) {
  case RELAY_SURFACES.staging.surface:
    await runWorkerSurface("staging", args);
    break;
  case RELAY_SURFACES.production.surface:
    await runWorkerSurface("production", args);
    break;
  case RELAY_SURFACES.stagingSchema.surface:
    await runSchemaSurface("staging", args);
    break;
  case RELAY_SURFACES.productionSchema.surface:
    await runSchemaSurface("production", args);
    break;
}
