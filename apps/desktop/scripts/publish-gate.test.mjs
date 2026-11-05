// Run with: node --test apps/desktop/scripts/publish-gate.test.mjs
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { BUILD_JOB, decide, ignoredPaths } from "./publish-gate.mjs";

const workflow = (name) => readFileSync(new URL(`../../../.github/workflows/${name}`, import.meta.url), "utf8");
const ignored = ignoredPaths(workflow("ci.yml"));

const push = (conclusion, created_at = "2026-11-05T00:00:00Z") => ({
  event: "push",
  created_at,
  status: conclusion ? "completed" : "in_progress",
  conclusion,
});

/** A stand-in for the `gh` CLI, answering from a small picture of the repository. */
function fakeGh({ ci = {}, files = {}, parents = {}, nightlies = {}, jobs = {} }) {
  return (...args) => {
    const path = args.find((arg) => arg.startsWith("repos/"));
    const sha = path.match(/head_sha=(\w+)/)?.[1];
    if (path.includes("workflows/ci.yml/runs")) return JSON.stringify({ workflow_runs: ci[sha] ?? [] });
    if (path.includes("workflows/nightly.yml/runs")) return JSON.stringify({ workflow_runs: nightlies[sha] ?? [] });
    const run = path.match(/actions\/runs\/(\w+)\/jobs/)?.[1];
    if (run) return JSON.stringify({ jobs: jobs[run] ?? [] });
    const commit = path.match(/commits\/(\w+)$/)?.[1];
    if (args.includes("--paginate")) return (files[commit] ?? []).join("\n");
    return `${parents[commit]}\n`;
  };
}

/** The gate's answer, with a clock that moves only when the gate sleeps. */
async function gate(world, { event = "schedule", ref = "refs/heads/main", sha = "head" } = {}) {
  let clock = 0;
  const sleeps = [];
  const result = await decide({
    event,
    ref,
    sha,
    repo: "o/r",
    gh: typeof world === "function" ? world : fakeGh(world),
    ignored,
    now: () => clock,
    sleep: async (ms) => {
      sleeps.push(ms);
      clock += ms;
    },
    log: () => {},
  });
  return { ...result, waitedMs: clock, sleeps };
}

const RELEASE = { event: "push", ref: "refs/tags/v1.0.0" };
const MINUTE = 60_000;

test("a nightly builds a commit whose CI passed", async () => {
  assert.equal((await gate({ ci: { head: [push("success")] } })).run, true);
});

for (const conclusion of ["failure", "cancelled", "timed_out"]) {
  test(`CI ${conclusion} stops a nightly and a release at once (#738)`, async () => {
    for (const options of [{}, RELEASE]) {
      const result = await gate({ ci: { head: [push(conclusion)] } }, options);
      assert.equal(result.run, false);
      assert.equal(result.stop, true);
      assert.equal(result.waitedMs, 0);
    }
  });
}

test("CI still running skips a nightly without failing it", async () => {
  const result = await gate({ ci: { head: [push(null)] } });
  assert.equal(result.run, false);
  assert.ok(!result.stop);
  assert.equal(result.waitedMs, 0);
});

test("a release waits for CI still running and goes on when it passes", async () => {
  let polls = 0;
  const gh = (...args) => {
    const running = polls++ < 3;
    return JSON.stringify({ workflow_runs: [push(running ? null : "success")], args });
  };
  const result = await gate(gh, RELEASE);
  assert.equal(result.run, true);
  assert.equal(result.waitedMs, 3 * MINUTE);
});

test("a release stops when CI is still running after 60 minutes", async () => {
  const result = await gate({ ci: { head: [push(null)] } }, RELEASE);
  assert.equal(result.stop, true);
  assert.equal(result.waitedMs, 60 * MINUTE);
});

test("a commit with code changes and no CI run stops a nightly; a release waits for one first", async () => {
  const world = { files: { head: ["crates/core/src/lib.rs"] } };
  assert.equal((await gate(world)).stop, true);
  const release = await gate(world, RELEASE);
  assert.equal(release.stop, true);
  assert.equal(release.waitedMs, 60 * MINUTE);
});

const docsOnTopOf = (conclusion) => ({
  files: { head: ["README.md"], docs: ["docs/user/install.md", ".github/workflows/nightly.yml"] },
  parents: { head: "docs", docs: "code" },
  ci: { code: [push(conclusion)] },
});

test("a commit CI ignores is judged by the nearest commit before it that CI ran on", async () => {
  assert.equal((await gate(docsOnTopOf("success"))).run, true);
  assert.equal((await gate(docsOnTopOf("failure"))).stop, true);
  const running = await gate(docsOnTopOf(null));
  assert.equal(running.run, false);
  assert.ok(!running.stop);
});

test("the latest run decides, so CI started again by hand can clear a cancelled push", async () => {
  const again = { ...push("success", "2026-11-05T01:00:00Z"), event: "workflow_dispatch" };
  assert.equal((await gate({ ci: { head: [again, push("cancelled")] } })).run, true);
  const pullRequest = { ...push("success", "2026-11-05T01:00:00Z"), event: "pull_request" };
  assert.equal((await gate({ ci: { head: [pullRequest, push("failure")] } })).stop, true);
});

test("a scheduled run skips a commit a good nightly already built", async () => {
  const result = await gate({
    ci: { head: [push("success")] },
    nightlies: { head: [{ id: 1 }] },
    jobs: { 1: [{ name: BUILD_JOB, conclusion: "success" }] },
  });
  assert.equal(result.run, false);
  assert.ok(!result.stop);
});

test("a nightly that only skipped doesn't count as having built the commit", async () => {
  const result = await gate({
    ci: { head: [push("success")] },
    nightlies: { head: [{ id: 1 }] },
    jobs: { 1: [{ name: "Anything new", conclusion: "success" }, { name: BUILD_JOB, conclusion: "skipped" }] },
  });
  assert.equal(result.run, true);
});

test("a run by hand on another branch publishes nothing, so CI isn't asked", async () => {
  assert.equal((await gate(() => assert.fail("no CI lookup expected"), { event: "workflow_dispatch", ref: "refs/heads/some-branch" })).run, true);
});

test("the gate names the build job as nightly.yml does", () => {
  assert.ok(workflow("nightly.yml").includes(`name: ${BUILD_JOB}\n`));
});

test("the paths CI ignores come from ci.yml", () => {
  for (const path of ["README.md", "crates/core/notes.md", "docs/a/b.png", "LICENSE", ".github/CODEOWNERS", ".github/ISSUE_TEMPLATE/bug.yml", ".github/workflows/nightly.yml"]) {
    assert.ok(ignored(path), path);
  }
  for (const path of ["crates/core/src/lib.rs", ".github/workflows/ci.yml", "tools/docs/run.sh", "LICENSE.txt"]) {
    assert.ok(!ignored(path), path);
  }
});
