import { afterAll, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { lineageVerdict } from "../scripts/lineage.mjs";

const repo = join(import.meta.dir, "..", "..");
const scratch = mkdtempSync(join(tmpdir(), "push-relay-deploy-test-"));
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

// A PATH holding only bun and git: whatever the entrypoint does, it cannot
// reach Wrangler and therefore cannot touch Cloudflare from these tests.
const safeBin = join(scratch, "bin");
mkdirSync(safeBin);
for (const tool of ["bun", "git"]) {
  const found = spawnSync("which", [tool], { encoding: "utf8" }).stdout.trim();
  symlinkSync(found, join(safeBin, tool));
}

function entrypoint(args: string[]) {
  return spawnSync("bun", ["scripts/deploy.mjs", ...args], {
    cwd: repo,
    encoding: "utf8",
    env: { PATH: safeBin, HOME: process.env.HOME ?? scratch, LANG: "C", GIT_TERMINAL_PROMPT: "0", TMPDIR: scratch },
  });
}

interface Surface {
  surface: string;
  target: string;
  triggers: string[];
  lineage: string;
  requiresScripts: string[];
  requiresTools: string[];
  requiresEnv: string[];
  obligations: Record<string, string>;
}

const TRIGGER_OBLIGATIONS: Record<string, string[]> = {
  irreversible: ["pre-mutation-proof", "independent-review"],
  authority: ["independent-review"],
  "published-identity": ["no-overwrite"],
  asynchronous: ["halt"],
};

test("--contract prints a v3 contract with every owed obligation answered", () => {
  const probe = entrypoint(["--contract"]);
  expect(probe.status).toBe(0);
  const contract = JSON.parse(probe.stdout) as { kind: string; surfaces: Surface[] };
  expect(contract.kind).toBe("takos.deploy-contract@v3");
  expect(contract.surfaces.map((entry) => entry.surface)).toEqual([
    "push-relay-staging",
    "push-relay",
    "push-relay-staging-schema",
    "push-relay-schema",
  ]);
  const scripts = JSON.parse(readFileSync(join(repo, "package.json"), "utf8")).scripts as Record<string, string>;
  for (const entry of contract.surfaces) {
    const owed = ["provenance", "post-conditions", "reversal", "failure-handling", ...entry.triggers.flatMap((t) => TRIGGER_OBLIGATIONS[t] ?? [])];
    for (const obligation of owed) expect(entry.obligations[obligation]?.length ?? 0).toBeGreaterThan(40);
    const answers = Object.values(entry.obligations).join("\n");
    for (const variable of entry.requiresEnv) expect(answers).toContain(variable);
    for (const script of entry.requiresScripts) expect(scripts[script]).toBeDefined();
    if (entry.triggers.includes("irreversible")) expect(entry.lineage).toBe("production-routine");
  }
  const production = contract.surfaces.find((entry) => entry.surface === "push-relay");
  expect(production?.lineage).toBe("production-routine");
});

test("every environment variable the deploy reads is declared by a surface", () => {
  const contract = JSON.parse(entrypoint(["--contract"]).stdout) as { surfaces: Surface[] };
  const declared = new Set(contract.surfaces.flatMap((entry) => entry.requiresEnv));
  const ambient = new Set(["HOME", "PATH", "TMPDIR", "NODE_ENV", "CI", "LANG"]);
  const files = ["scripts/deploy.mjs", "relay/scripts/deploy-relay.mjs", "relay/scripts/lineage.mjs", "relay/scripts/smoke.ts"];
  const reads = new Set<string>();
  for (const file of files) {
    for (const match of readFileSync(join(repo, file), "utf8").matchAll(/process\.env\.([A-Z][A-Z0-9_]*)/gu)) {
      if (match[1]) reads.add(match[1]);
    }
  }
  const undeclared = [...reads].filter((name) => !declared.has(name) && !ambient.has(name));
  expect(undeclared).toEqual([]);
});

test("the package scripts hold no second way to mutate a provider", () => {
  const scripts = JSON.parse(readFileSync(join(repo, "package.json"), "utf8")).scripts as Record<string, string>;
  const second =
    /\bwrangler(?:@\S+)?\s+(?:[^\n|&;]*\s)?(?:deploy|pages\s+deploy|versions\s+(?:upload|deploy)|triggers\s+deploy|secret\s+put)\b|\b(?:npm|bun|pnpm|yarn)\s+publish\b/u;
  for (const [name, command] of Object.entries(scripts)) {
    if (name !== "deploy") expect(second.test(command)).toBe(false);
  }
});

// The seven git states of takos-control's lineage corpus, and the verdict each
// class owes there (engineering.policy.json → deploy.lineage).
function git(cwd: string, ...args: string[]) {
  const result = spawnSync("git", args, {
    cwd,
    encoding: "utf8",
    env: { ...process.env, GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@example.invalid", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@example.invalid" },
  });
  if (result.status !== 0) throw new Error(`git ${args.join(" ")}: ${result.stderr}`);
  return result.stdout.trim();
}

function commit(cwd: string, file: string, contents: string) {
  writeFileSync(join(cwd, file), contents);
  git(cwd, "add", "-A");
  git(cwd, "commit", "-q", "-m", file);
}

const EXPECTED: Record<string, Record<string, "accept" | "refuse">> = {
  "clean-at-origin-main": { integration: "accept", "production-routine": "accept", "published-identity": "refuse" },
  "dirty-worktree": { integration: "accept", "production-routine": "refuse", "published-identity": "refuse" },
  "unpushed-commit": { integration: "accept", "production-routine": "refuse", "published-identity": "refuse" },
  "ancestor-of-origin-main": { integration: "accept", "production-routine": "accept", "published-identity": "refuse" },
  "detached-head": { integration: "accept", "production-routine": "refuse", "published-identity": "refuse" },
  "non-main-branch-pushed": { integration: "accept", "production-routine": "refuse", "published-identity": "refuse" },
  "local-tag-not-on-origin": { integration: "accept", "production-routine": "accept", "published-identity": "refuse" },
};

function materialize(id: string): string {
  const root = join(scratch, "corpus", id);
  const bare = join(root, "origin.git");
  const work = join(root, "checkout");
  mkdirSync(root, { recursive: true });
  git(dirname(bare), "init", "-q", "--bare", "--initial-branch=main", bare);
  git(root, "init", "-q", "--initial-branch=main", work);
  git(work, "remote", "add", "origin", bare);
  commit(work, "README.md", "base\n");
  git(work, "push", "-q", "origin", "main");
  switch (id) {
    case "dirty-worktree":
      writeFileSync(join(work, "README.md"), "modified\n");
      break;
    case "unpushed-commit":
      commit(work, "ahead.txt", "ahead\n");
      break;
    case "ancestor-of-origin-main": {
      const base = git(work, "rev-parse", "HEAD");
      commit(work, "newer.txt", "newer\n");
      git(work, "push", "-q", "origin", "main");
      git(work, "checkout", "-q", "-B", "main", base);
      break;
    }
    case "detached-head":
      git(work, "checkout", "-q", "--detach", "HEAD");
      break;
    case "non-main-branch-pushed":
      git(work, "checkout", "-q", "-b", "feature/lineage");
      commit(work, "feature.txt", "feature\n");
      git(work, "push", "-q", "origin", "feature/lineage");
      break;
    case "local-tag-not-on-origin":
      git(work, "tag", "v9.9.9");
      break;
  }
  git(work, "fetch", "-q", "origin");
  return work;
}

test("the deploy's lineage function gives the corpus verdicts, also through --lineage-selftest", () => {
  for (const id of Object.keys(EXPECTED)) materialize(id);
  for (const lineageClass of ["integration", "production-routine", "published-identity"]) {
    const selftest = entrypoint(["--lineage-selftest", join(scratch, "corpus"), lineageClass]);
    expect(selftest.status).toBe(0);
    const document = JSON.parse(selftest.stdout) as { kind: string; verdicts: Record<string, string> };
    expect(document.kind).toBe("takos.lineage-corpus@v1");
    for (const [id, verdicts] of Object.entries(EXPECTED)) {
      expect(`${id}:${document.verdicts[id]}`).toBe(`${id}:${verdicts[lineageClass]}`);
      if (lineageClass === "published-identity") {
        expect(lineageVerdict(lineageClass, join(scratch, "corpus", id, "checkout"), { tag: "v9.9.9" }).verdict).toBe("refuse");
      }
    }
  }
});

test("surfaces refuse before touching Cloudflare when their inputs are missing", () => {
  const staging = entrypoint(["push-relay-staging"]);
  expect(staging.status).toBe(1);
  expect(staging.stderr).toContain("deploy blocked: PUSH_RELAY_STAGING_D1_DATABASE_ID is not set");

  for (const surface of ["push-relay", "push-relay-staging-schema", "push-relay-schema"]) {
    const refused = entrypoint([surface]);
    expect(refused.status).toBe(1);
    expect(refused.stderr).toContain("deploy blocked:");
  }

  const unknown = entrypoint(["push-relay-everything"]);
  expect(unknown.status).toBe(1);
  expect(unknown.stderr).toContain("known surfaces: push-relay-staging, push-relay, push-relay-staging-schema, push-relay-schema");
});
