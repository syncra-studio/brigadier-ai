import assert from "node:assert/strict";
import { test } from "node:test";

import { ciWatches, ciWords, heldMerge, mergeWords, slotRows } from "@/app/conversation/activity/slotView";
import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import type { CiRun, OrchestratorStep, OrchestratorStepKind, ReviewRun, Task } from "@/ipc/generated";

let at = 0;
const step = (kind: OrchestratorStepKind): OrchestratorStep => {
  at += 10;
  return { requestId: null, kind, atMs: at, position: at };
};
const run = (jobsDone: number, jobsTotal: number, workflow = "CI"): CiRun =>
  ({ workflow, status: jobsDone === jobsTotal ? "completed" : "in_progress", conclusion: "", url: `https://github.com/o/n/actions/runs/${workflow}`, jobsDone, jobsTotal });
const watch = (sha: string, runs: CiRun[], result?: string, branch = "main"): OrchestratorStep =>
  step({ type: "ciWatch", repo: "o/n", branch, sha, waiting: runs.length === 0 && result === undefined, runs, ...(result ? { result } : {}) });

test("the CI pill holds every watch still checking, each by its latest row", () => {
  const steps = [
    watch("aaaa1111", []),
    watch("bbbb2222", [], undefined, "fix"),
    watch("aaaa1111", [run(1, 3)]),
    watch("bbbb2222", [run(0, 2)], undefined, "fix"),
  ];
  const both = ciWatches(steps);
  assert.deepEqual(both.map((w) => [w.sha, w.branch]), [["aaaa1111", "main"], ["bbbb2222", "fix"]]);
  assert.equal(ciWords(both), "Checking CI for 2 pushes · 1 of 5 jobs done");
  // One finishes, the other keeps running: the pill stays, with only the one left.
  const one = ciWatches([...steps, watch("aaaa1111", [run(3, 3)], "passed"), watch("bbbb2222", [run(1, 2)], undefined, "fix")]);
  assert.deepEqual(one.map((w) => w.sha), ["bbbb2222"]);
  assert.equal(ciWords(one), "Checking CI · 1 of 2 jobs done");
  // Both over: no pill.
  assert.deepEqual(ciWatches([...steps, watch("aaaa1111", [], "no runs"), watch("bbbb2222", [run(2, 2)], "failed", "fix")]), []);
});

test("a watch waits for its first run in plain words", () => {
  assert.equal(ciWords(ciWatches([watch("cccc3333", [])])), "Waiting for CI to start…");
  assert.equal(ciWords(ciWatches([watch("cccc3333", []), watch("dddd4444", [])])), "Waiting for CI to start for 2 pushes…");
});

const base = Object.values(night.tasks)[0] as unknown as Task;
const tasks = (states: Record<string, Task["state"]>): Record<string, Task> =>
  Object.fromEntries(Object.entries(states).map(([id, state]) => [id, { ...base, id, state }]));
const held = (askedIn = "m1", waiting = ["t1", "t2"]): OrchestratorStep =>
  step({ type: "mergeHeld", branch: "brigadier/s", base: "main", askedIn, consentedAtMs: 0, condition: "workersFinish", waiting, tip: "abc" });
const review = (state: ReviewRun["state"]["type"]) =>
  ({ kind: "code", notify: { type: "orchestrator" }, state: { type: state } }) as ReviewRun;

test("the merge pill counts the writers it still waits for, then the review", () => {
  const steps = [held()];
  const running = heldMerge(steps, tasks({ t1: "running", t2: "landing" }), []);
  assert.equal(running?.waiting, 2);
  assert.equal(mergeWords(running!), "Merging after 2 workers finish");
  // A reported task hasn't landed yet: it still counts.
  assert.equal(mergeWords(heldMerge(steps, tasks({ t1: "landed", t2: "reported" }), [])!), "Merging after 1 worker finishes");
  assert.equal(mergeWords(heldMerge(steps, tasks({ t1: "landed", t2: "failed" }), [review("running")])!), "Merging after the review");
  assert.equal(mergeWords(heldMerge(steps, tasks({ t1: "landed", t2: "landed" }), [review("clean")])!), "Merging…");
});

test("the merge pill goes once the merge lands, is cancelled or is stopped", () => {
  const all = tasks({ t1: "running" });
  assert.equal(heldMerge([held(), step({ type: "merged", branch: "brigadier/s", base: "main", commits: 2, askedIn: "m1" })], all, []), null);
  assert.equal(heldMerge([held(), step({ type: "mergeCancelled", askedIn: "m1" })], all, []), null);
  assert.equal(heldMerge([held(), step({ type: "mergeStopped", askedIn: "m1", reason: "task-5 failed" })], all, []), null);
  // Another consent's Cancel leaves it; a later hold after a Cancel shows again.
  assert.notEqual(heldMerge([held("m2"), step({ type: "mergeCancelled", askedIn: "m1" })], all, []), null);
  assert.notEqual(heldMerge([held(), step({ type: "mergeCancelled", askedIn: "m1" }), held("m3")], all, []), null);
  assert.equal(heldMerge([], all, []), null);
});

test("the slot stacks its rows newest last", () => {
  assert.deepEqual(slotRows({ workers: 30, ci: 10, merge: 20 }), ["ci", "merge", "workers"]);
  assert.deepEqual(slotRows({ merge: 5 }), ["merge"]);
  assert.deepEqual(slotRows({}), []);
});
