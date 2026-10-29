// Which source a deploy of each lineage class may proceed from.
//
// The classes and their predicates are takos-control's
// `engineering.policy.json` → `deploy.lineage`. This copies the shape of
// control's reference predicate on purpose: it has to run in this repository's
// own process, against the checkout this repository is about to build, and the
// very same function answers `--lineage-selftest` so control can run it over
// its corpus of git states.

import { execFileSync } from "node:child_process";
import { existsSync, readdirSync } from "node:fs";
import { join } from "node:path";

export const LINEAGE_CLASSES = ["integration", "rehearsal", "production-routine", "published-identity"];

function git(args, cwd) {
  try {
    return {
      ok: true,
      out: execFileSync("git", args, {
        cwd,
        encoding: "utf8",
        stdio: ["ignore", "pipe", "ignore"],
        env: { ...process.env, GIT_TERMINAL_PROMPT: "0" },
      }).trim(),
    };
  } catch {
    return { ok: false, out: "" };
  }
}

/**
 * @param {string} lineageClass
 * @param {string} cwd
 * @param {{ tag?: string | null, defaultBranch?: string }} [options]
 * @returns {{ verdict: "accept" | "refuse", why: string }}
 */
export function lineageVerdict(lineageClass, cwd, { tag = null, defaultBranch = "main" } = {}) {
  if (lineageClass === "integration" || lineageClass === "rehearsal") {
    return { verdict: "accept", why: "any source is admissible in this environment" };
  }
  if (!LINEAGE_CLASSES.includes(lineageClass)) {
    return { verdict: "refuse", why: `unknown lineage class ${lineageClass}` };
  }

  const status = git(["status", "--porcelain=v1", "--untracked-files=no"], cwd);
  if (!status.ok) return { verdict: "refuse", why: "not a git checkout" };
  if (status.out.length > 0) {
    return { verdict: "refuse", why: "the worktree is dirty, so the built bytes belong to no commit" };
  }
  const branch = git(["symbolic-ref", "--quiet", "--short", "HEAD"], cwd);
  if (!branch.ok || branch.out.length === 0) {
    return { verdict: "refuse", why: "detached HEAD; nothing names what was built" };
  }
  if (branch.out !== defaultBranch) {
    return { verdict: "refuse", why: `on ${branch.out}, not the default branch ${defaultBranch}` };
  }
  git(["fetch", "--quiet", "origin", defaultBranch], cwd);
  const remote = git(["rev-parse", "--verify", `refs/remotes/origin/${defaultBranch}`], cwd);
  if (!remote.ok) {
    return { verdict: "refuse", why: `origin/${defaultBranch} is unknown, so nobody else can obtain this source` };
  }
  const head = git(["rev-parse", "HEAD"], cwd);
  const reachable = head.out === remote.out || git(["merge-base", "--is-ancestor", head.out, remote.out], cwd).ok;
  if (!reachable) {
    return { verdict: "refuse", why: `HEAD is neither origin/${defaultBranch} nor an ancestor of it` };
  }
  if (lineageClass === "production-routine") {
    return { verdict: "accept", why: `clean, on ${defaultBranch}, reachable from origin` };
  }

  if (!tag) return { verdict: "refuse", why: "no tag named; a published identity must be a tag on origin" };
  const remoteTag = git(["ls-remote", "--tags", "origin", `refs/tags/${tag}`], cwd);
  if (!remoteTag.ok || remoteTag.out.length === 0) {
    return { verdict: "refuse", why: `tag ${tag} does not exist on origin` };
  }
  const resolved = remoteTag.out.split("\n")[0].split("\t")[0];
  const peeled = git(["rev-parse", `${tag}^{commit}`], cwd);
  if (resolved !== head.out && peeled.out !== head.out) {
    return { verdict: "refuse", why: `tag ${tag} on origin does not point at HEAD` };
  }
  return { verdict: "accept", why: `tag ${tag} exists on origin at HEAD` };
}

/**
 * Runs the predicate over control's materialized corpus: one directory per
 * case, each holding a `checkout`. This repository publishes no tag, so the
 * published-identity class is asked with none and refuses everywhere.
 */
export function runLineageSelfTest(corpusRoot, lineageClass) {
  const verdicts = {};
  for (const id of readdirSync(corpusRoot).sort()) {
    const checkout = join(corpusRoot, id, "checkout");
    if (!existsSync(checkout)) continue;
    verdicts[id] = lineageVerdict(lineageClass, checkout).verdict;
  }
  return { kind: "takos.lineage-corpus@v1", lineage: lineageClass, verdicts };
}
