import assert from "node:assert/strict";
import { test } from "node:test";

import { ciPill, ciWatches } from "@/app/conversation/activity/slotView";
import type { CiRun, OrchestratorStep, OrchestratorStepKind } from "@/ipc/generated";

let at = 0;
const step = (kind: OrchestratorStepKind): OrchestratorStep => {
  at += 10;
  return { requestId: null, kind, atMs: at, position: at };
};
const run = (jobsDone: number, jobsTotal: number, workflow = "CI"): CiRun =>
  ({ workflow, status: jobsDone === jobsTotal ? "completed" : "in_progress", conclusion: "", url: `https://github.com/o/n/actions/runs/${workflow}`, jobsDone, jobsTotal });
const watch = (sha: string, runs: CiRun[], result?: string, branch = "main"): OrchestratorStep =>
  step({ type: "ciWatch", repo: "o/n", branch, sha, waiting: runs.length === 0 && result === undefined, runs, ...(result ? { result } : {}) });
const ended = (workflow: string, conclusion: string, jobs = 2): CiRun =>
  ({ workflow, status: "completed", conclusion, url: `https://github.com/o/n/actions/runs/${workflow}`, jobsDone: jobs, jobsTotal: jobs });
const ciWords = (watches: Parameters<typeof ciPill>[0]) => ciPill(watches).words;

test("CI's words take every watch still checking, each by its latest row", () => {
  const steps = [
    watch("aaaa1111", []),
    watch("bbbb2222", [], undefined, "fix"),
    watch("aaaa1111", [run(1, 3)]),
    watch("bbbb2222", [run(0, 2)], undefined, "fix"),
  ];
  const both = ciWatches(steps);
  assert.deepEqual(both.map((w) => [w.sha, w.branch]), [["aaaa1111", "main"], ["bbbb2222", "fix"]]);
  assert.equal(ciWords(both), "Checking CI for 2 pushes · 1 of 5 jobs done");
  // One finishes, the other keeps running: the words count only the one left.
  const one = ciWatches([...steps, watch("aaaa1111", [run(3, 3)], "passed"), watch("bbbb2222", [run(1, 2)], undefined, "fix")]);
  assert.deepEqual(one.map((w) => [w.sha, w.result]), [["aaaa1111", "passed"], ["bbbb2222", null]]);
  assert.equal(ciWords(one), "Checking CI · 1 of 2 jobs done");
  // Both over: they stay, with how each ended.
  const over = ciWatches([...steps, watch("aaaa1111", [], "no runs"), watch("bbbb2222", [run(2, 2)], "failed", "fix")]);
  assert.deepEqual(over.map((w) => w.result), ["no runs", "failed"]);
  assert.equal(ciWords(over), "CI failed");
});

test("a watch waits for its first run in plain words", () => {
  assert.equal(ciWords(ciWatches([watch("cccc3333", [])])), "Waiting for CI to start…");
  assert.equal(ciWords(ciWatches([watch("cccc3333", []), watch("dddd4444", [])])), "Waiting for CI to start for 2 pushes…");
});

test("a run whose jobs are all done still checks until GitHub completes it", () => {
  const busy: CiRun = { workflow: "CI", status: "in_progress", conclusion: "", url: "https://github.com/o/n/actions/runs/1", jobsDone: 1, jobsTotal: 1 };
  const pill = ciPill(ciWatches([watch("eeee5555", [busy])]));
  assert.equal(pill.state, "checking");
  assert.doesNotMatch(pill.words, /1 of 1 jobs done/);
  assert.equal(pill.words, "Checking CI · finishing up…");
  // Another run still going: the count reads, the finished one counted whole.
  assert.deepEqual(ciPill(ciWatches([watch("eeee5555", [busy, run(1, 3, "Lint"), ended("Docs", "success")])])), {
    state: "checking",
    words: "Checking CI · 4 of 6 jobs done",
  });
});

test("a failed run fails the pill at once, naming its workflow", () => {
  const failed = ciPill(ciWatches([watch("ffff6666", [ended("test", "failure"), run(1, 3, "Lint")])]));
  assert.deepEqual(failed, { state: "failed", words: "CI failed · test" });
  assert.deepEqual(ciPill(ciWatches([watch("ffff6666", [ended("test", "timed_out"), ended("lint", "failure"), ended("build", "cancelled")])])), {
    state: "failed",
    words: "CI failed · 2 workflows",
  });
});

test("a cancelled run, with none failed, says cancelled: never a silent failure", () => {
  // GitHub cancels a run whose job hit its time limit saving the cache; the daemon's result is failed.
  assert.deepEqual(ciPill(ciWatches([watch("a9716757", [ended("CI", "cancelled")], "failed")])), { state: "cancelled", words: "CI cancelled · CI" });
  assert.deepEqual(ciPill(ciWatches([watch("a9716757", [ended("CI", "cancelled"), ended("test", "failure")], "failed")])), { state: "failed", words: "CI failed · test" });
});

test("every run passed while the daemon waits for a late one: passed, still looking", () => {
  const settling = ciPill(ciWatches([watch("abab7777", [ended("CI", "success"), ended("Lint", "skipped")])]));
  assert.deepEqual(settling, { state: "passed", words: "CI passed · checking for more runs…" });
});

test("a watch's result stays once it is over, in its own words", () => {
  const steps = [watch("cdcd8888", [run(1, 2)]), watch("cdcd8888", [ended("CI", "success")]), watch("cdcd8888", [ended("CI", "success")], "passed")];
  const endedAt = steps.at(-1)!.atMs;
  const over = ciWatches(steps);
  assert.equal(over.length, 1);
  assert.equal(over[0]?.endedAtMs, endedAt);
  assert.deepEqual(ciPill(over), { state: "passed", words: "CI passed" });
  const words = (runs: CiRun[], result: string) => ciPill(ciWatches([watch("efef9999", runs, result)]));
  assert.deepEqual(words([ended("test", "failure")], "failed"), { state: "failed", words: "CI failed · test" });
  assert.deepEqual(words([], "no runs"), { state: "ended", words: "No CI runs" });
  assert.deepEqual(words([run(0, 2)], "stopped"), { state: "ended", words: "Stopped watching CI" });
});
