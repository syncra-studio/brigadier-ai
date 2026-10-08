import assert from "node:assert/strict";
import { test } from "node:test";

import { taskState, workerName } from "@/app/conversation/rowWords";
import { taskActivityLines, taskActivityTicks, taskWaitWords } from "@/app/conversation/taskActivity";
import { workerDone, workerPreview, workerState, workerWorking } from "@/app/conversation/workerPresentation";
import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import type { Task } from "@/ipc/generated";

const base = Object.values(night.tasks)[0] as unknown as Task;
const task = (patch: Partial<Task> = {}): Task => ({ ...base, state: "running", createdAtMs: 1000, updatedAtMs: 1000,
  attempts: [], quotaWait: null, blockedReason: null, candidate: null, ...patch });
const line = (worker: Task, activity = "Thinking…") => taskActivityLines({ task: worker, activity }, 41000);

test("live activity includes elapsed and nonzero per-worker diff; completed rows have no activity", () => {
  assert.equal(taskActivityLines({ task: task(), activity: "$ pnpm typecheck", diff: { files: [], insertions: 209, deletions: 102 } }, 193000).first,
    "$ pnpm typecheck · 3m 12s · +209 −102");
  for (const state of ["done", "landed", "failed", "rejected", "stopped"] as const) assert.equal(line(task({ state })).first, "");
  assert.equal(line(task()).firstWorking, true);
  assert.equal(line(task(), "Waiting for approval").firstWorking, false);
  assert.equal(line(task(), "Retrying: quota").firstWorking, false);
});

test("waits override stale work and never claim Working or shimmer", () => {
  for (const state of ["blocked", "queued", "paused"] as const) {
    const worker = task({ state, blockedReason: state === "paused" ? "Waiting for a dependency" : null });
    assert.match(line(worker, "Editing 3 files").first, /^(Waiting|Queued)/);
    assert.equal(line(worker).firstWorking, false);
    assert.doesNotMatch(taskState(worker).word, /Working/);
  }
  assert.equal(taskWaitWords(task({ state: "blocked", blockedReason: "Waiting for a free worker" })), "Waiting for a free worker");
  assert.equal(taskWaitWords(task({ state: "queued", blockedReason: "waiting for step 1" })), "Queued: waiting for step 1");
  const quota = task({ quotaWait: { reason: "Codex weekly quota resets tomorrow", sinceMs: 11000, resetsAtMs: null, rule: null, ranking: null } });
  assert.equal(line(quota, "Writing…").first, "Waiting for Codex quota: Codex weekly quota resets tomorrow · 30s");
  assert.equal(taskState(quota).word, "Waiting for Codex quota");
});

test("a new attempt uses its own start time", () => {
  const worker = task({ attempts: [{ route: base.route, startedAtMs: 11000, endedAtMs: null, end: null }] });
  assert.equal(line(worker, "Editing 2 files").first, "Editing 2 files · 30s");
});

test("landing reads Landing; ready to land keeps its warning tone, including Held during a run", () => {
  assert.deepEqual(taskState(task({ state: "landing" })), { word: "Landing", tone: "live" });
  assert.deepEqual(taskState(task({ state: "readyToLand", run: null })), { word: "Ready to land", tone: "warning" });
  assert.ok(base.run);
  const held = task({ state: "readyToLand", run: base.run });
  assert.deepEqual(taskState(held), { word: "Held", tone: "warning" });
  assert.equal(taskWaitWords(held), "Held");
  assert.match(line(held).first, /^Held · /);
});

test("settled workers freeze elapsed at attempt end and do not subscribe to the clock", () => {
  for (const state of ["reported", "readyToLand"] as const) {
    const worker = task({ state, updatedAtMs: 39000,
      attempts: [{ route: base.route, startedAtMs: 11000, endedAtMs: 31000, end: null }] });
    assert.match(line(worker).first, / · 20s$/);
    assert.deepEqual(line(worker), taskActivityLines({ task: worker, activity: "Thinking…" }, 141000));
    assert.equal(taskActivityTicks(worker), false);
    assert.match(line(task({ state, updatedAtMs: 21000 })).first, / · 20s$/);
  }
});

test("a worker that reported says it finished and hands back", () => {
  const worker = task({ state: "reported", attempts: [{ route: base.route, startedAtMs: 11000, endedAtMs: 31000, end: null }] });
  assert.equal(line(worker, "Editing 3 files").first, "Finished, handing back · 20s");
  assert.equal(line(worker).firstWorking, false);
});

test("quota waits tick from sinceMs even after the worker's attempt ended", () => {
  const worker = task({ state: "paused", quotaWait: { reason: "Codex quota", sinceMs: 11000, resetsAtMs: null, rule: null, ranking: null },
    attempts: [{ route: base.route, startedAtMs: 1000, endedAtMs: 10000, end: null }] });
  assert.match(line(worker).first, / · 30s$/);
  assert.equal(taskActivityTicks(worker), true);
  assert.equal(taskActivityTicks(task({ state: "done" })), false);
});

test("a user pause says Paused and freezes at updatedAtMs without keeping the clock", () => {
  for (const blockedReason of [null, "", "  "]) {
    const worker = task({ state: "paused", updatedAtMs: 31000, blockedReason,
      attempts: [{ route: base.route, startedAtMs: 11000, endedAtMs: 21000, end: null }] });
    assert.equal(taskWaitWords(worker), "Paused");
    assert.deepEqual(taskState(worker), { word: "Paused", tone: "quiet" });
    assert.equal(line(worker, "Editing 3 files").first, "Paused · 20s");
    assert.equal(line(worker).firstWorking, false);
    assert.equal(taskActivityTicks(worker), false);
    assert.deepEqual(line(worker), taskActivityLines({ task: worker, activity: "Thinking…" }, 141000));
    assert.equal(taskActivityTicks({ ...worker, state: "running" }), true);
  }
  assert.equal(taskActivityTicks(task({ state: "paused", blockedReason: "Waiting for a dependency" })), true);
});

test("a worker of a request's flow is named by its part and phase, the outline reviewer too", () => {
  assert.equal(workerName({}, task({ role: "lead", phase: 1 })), "Break down the overnight");
  assert.equal(workerName({}, task({ role: "reviewer", phase: 2 })), "Break down the overnight");
  assert.equal(workerName({}, task({ role: "reviewer", phase: null })), "Break down the overnight");
});

test("a worker in the user's terminal waits on the user, in plain words everywhere", () => {
  const worker = task({ state: "takenOver", attempts: [{ route: base.route, startedAtMs: 11000, endedAtMs: null, end: null }] });
  assert.deepEqual(taskState(worker), { word: "In your terminal", tone: "warning" });
  assert.equal(taskWaitWords(worker), "Working in your terminal");
  assert.equal(line(worker, "Editing 3 files").first, "Working in your terminal · 30s");
  assert.equal(line(worker).firstWorking, false);
  assert.equal(workerDone(worker), false);
  assert.equal(workerWorking(worker), false);
  assert.equal(workerState(worker), "is in your terminal");
  assert.equal(workerPreview(worker), "Working in your terminal");
});
