import assert from "node:assert/strict";
import { test } from "node:test";

import { taskState } from "@/app/conversation/rowWords";
import { taskActivityLines, taskActivityTicks, taskWaitWords } from "@/app/conversation/taskActivity";
import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import type { Gate, Task } from "@/ipc/generated";

const base = Object.values(night.tasks)[0] as unknown as Task;
const task = (patch: Partial<Task> = {}): Task => ({ ...base, state: "running", createdAtMs: 1000, updatedAtMs: 1000,
  attempts: [], quotaWait: null, blockedReason: null, gate: null, candidate: null, fixRounds: 0, ...patch });
const line = (worker: Task, activity = "Thinking…") => taskActivityLines({ task: worker, activity }, [], 41000);

test("live activity includes elapsed and nonzero per-worker diff; completed rows have no activity", () => {
  assert.equal(taskActivityLines({ task: task(), activity: "$ pnpm typecheck", diff: { files: [], insertions: 209, deletions: 102 } }, [], 193000).first,
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

test("current gate names reviewer and verifier activities, waits, and extra checks", () => {
  const gate: Gate = { round: 1, commit: null, outcome: null, relanding: false, retry: false, overridden: false, findings: [],
    rebased: false, verificationScope: { type: "full", reason: "Worker activity test" },
    members: ["review", "verify", "review"].map((role, i) => ({ taskId: `check-${i}`, role: role as "review" | "verify", result: null, avoid: [] })) };
  const reviewer = task({ id: "check-0", route: { ...base.route, choice: { ...base.route.choice, provider: "codex" } } });
  const verifier = task({ id: "check-1", createdAtMs: 31000 });
  const owner = task({ state: "reviewing", gate });
  const lines = taskActivityLines({ task: owner }, [{ task: reviewer, activity: "Thinking…" }, { task: verifier, activity: "$ cargo test -p core" }], 71000);
  assert.equal(lines.first, "Codex reviewing the diff: Thinking… · 1m 10s");
  assert.equal(lines.second, "Verifier: $ cargo test -p core · 40s · +1 more check");
  assert.equal(lines.secondWorking, true);
  const withDiff = taskActivityLines({ task: owner, diff: { files: [], insertions: 12, deletions: 3 } }, [{ task: reviewer }], 71000);
  assert.match(withDiff.first, / · \+12 −3$/);
  assert.match(line(task({ state: "reviewing", gate: { ...gate, members: [] } })).first, /^Waiting for checks/);
  const waiting = taskActivityLines({ task: owner }, [{ task: task({ ...reviewer, state: "blocked", blockedReason: "Waiting for quota" }) }], 71000);
  assert.match(waiting.first, /Waiting for quota/);
  assert.equal(waiting.firstWorking, false);
  assert.equal(waiting.second, "Verifier: waiting to start · +1 more check");
});

test("fixes are labelled and a new attempt uses its own start time", () => {
  const worker = task({ fixRounds: 1, attempts: [{ route: base.route, startedAtMs: 11000, endedAtMs: null, end: null }] });
  assert.equal(line(worker, "Editing 2 files").first, "Fixing review findings (round 1): Editing 2 files · 30s");
});

test("approval and landing retain their warning tones, including Held during a run", () => {
  assert.deepEqual(taskState(task({ state: "awaitingApproval" })), { word: "Waiting for you", tone: "warning" });
  assert.deepEqual(taskState(task({ state: "readyToLand", run: null })), { word: "Ready to land", tone: "warning" });
  assert.ok(base.run);
  const held = task({ state: "readyToLand", run: base.run });
  assert.deepEqual(taskState(held), { word: "Held", tone: "warning" });
  assert.equal(taskWaitWords(held), "Held");
  assert.match(line(held).first, /^Held · /);
});

test("settled workers freeze elapsed at attempt end and do not subscribe to the clock", () => {
  for (const state of ["reported", "awaitingApproval", "readyToLand"] as const) {
    const worker = task({ state, updatedAtMs: 39000,
      attempts: [{ route: base.route, startedAtMs: 11000, endedAtMs: 31000, end: null }] });
    assert.match(line(worker).first, / · 20s$/);
    assert.deepEqual(line(worker), taskActivityLines({ task: worker, activity: "Thinking…" }, [], 141000));
    assert.equal(taskActivityTicks(worker), false);
    assert.match(line(task({ state, updatedAtMs: 21000 })).first, / · 20s$/);
  }
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
    assert.deepEqual(line(worker), taskActivityLines({ task: worker, activity: "Thinking…" }, [], 141000));
    assert.equal(taskActivityTicks({ ...worker, state: "running" }), true);
  }
  assert.equal(taskActivityTicks(task({ state: "paused", blockedReason: "Waiting for a dependency" })), true);
});

test("checker overflow uses singular for one hidden check and plural for two", () => {
  const gate: Gate = { round: 1, commit: null, outcome: null, relanding: false, retry: false, overridden: false, findings: [], members: [],
    rebased: false, verificationScope: { type: "full", reason: "Worker activity test" } };
  for (const [count, expected] of [[3, "Verifier: waiting to start · +1 more check"], [4, "Verifier: waiting to start · +2 more checks"]] as const) {
    const members = Array.from({ length: count }, (_, index) => ({ taskId: `check-${index}`, role: "verify" as const, result: null, avoid: [] }));
    const owner = task({ state: "reviewing", gate: { ...gate, members } });
    assert.equal(taskActivityLines({ task: owner }, [], 41000).second, expected);
  }
});
