import assert from "node:assert/strict";
import { test } from "node:test";

import { finishedCounts, finishedWords, QUIET_MS, rowNames, unmergedRows, workerStateOf } from "@/app/conversation/activity/workerState";
import type { WorkerRow } from "@/ipc/generated";
import { formatTime } from "@/lib/format";

const NOW = 10_000_000;
const row = (patch: Partial<WorkerRow> = {}): WorkerRow => ({
  id: "t1", number: 1, title: "Fix the login page", agent: "claude", model: null, startedAtMs: 0,
  working: true, lastEnd: null, gone: false, lastEventAtMs: NOW, limit: null, freshContext: false, waiting: [], mcp: [],
  branch: "w/1", base: "main", diff: null, ahead: 0, merged: false, ...patch,
});
const word = (patch: Partial<WorkerRow>, now = NOW) => workerStateOf(row(patch), now).word;
const changed = { files: [{ path: "a.ts", insertions: 3, deletions: 1, binary: false, change: "update" }], insertions: 3, deletions: 1 } as WorkerRow["diff"];

test("a worker that waits on the user needs you, before anything else", () => {
  assert.equal(word({ waiting: [{ kind: "question", what: "Which port?", card: "q1" }], limit: { kind: "usageWindow", resetsAtMs: NOW, window: null } }), "Needs you");
  assert.equal(word({ working: false, lastEnd: "done", waiting: [{ kind: "signIn", what: "linear", card: null }] }), "Needs you");
});

test("a usage limit reads Limit until its reset, or Limit without a time", () => {
  const at = NOW + 3_600_000;
  assert.equal(word({ limit: { kind: "usageWindow", resetsAtMs: at, window: "five_hour" } }), `Limit until ${formatTime(at)}`);
  assert.equal(word({ limit: { kind: "credits", resetsAtMs: null, window: null } }), "Limit");
});

test("a compaction in the open turn reads Fresh context; once the turn ends it no longer does", () => {
  assert.equal(word({ freshContext: true }), "Fresh context");
  assert.equal(word({ freshContext: true, working: false, lastEnd: "done" }), "Done");
});

test("a worker at work turns quiet after 20 minutes of nothing, by the real minutes", () => {
  assert.equal(word({ lastEventAtMs: NOW - (QUIET_MS - 1000) }), "Working");
  assert.equal(word({ lastEventAtMs: NOW - QUIET_MS }), "Quiet 20m");
  assert.equal(word({ lastEventAtMs: NOW - 34 * 60_000 - 59_000 }), "Quiet 34m");
  // Not started its first turn yet: it is at it, never quiet.
  assert.equal(word({ working: false, lastEventAtMs: NOW - 2 * QUIET_MS }), "Working");
});

test("how its last turn ended: failed (or cut off, or gone without a done end), stopped", () => {
  assert.equal(word({ working: false, lastEnd: "failed" }), "Failed");
  assert.equal(word({ working: false, lastEnd: "interrupted" }), "Failed");
  assert.equal(word({ working: false, lastEnd: null, gone: true }), "Failed");
  assert.equal(word({ working: false, lastEnd: "stopped" }), "Stopped");
  assert.equal(word({ working: false, lastEnd: "stopped", gone: true, merged: true }), "Stopped");
});

test("git says the rest: Merged, Done · not merged (commits ahead or changes), Done", () => {
  assert.equal(word({ working: false, lastEnd: "done", merged: true, ahead: 0 }), "Merged");
  assert.equal(word({ working: false, lastEnd: "done", ahead: 2 }), "Done · not merged");
  assert.equal(word({ working: false, lastEnd: "done", diff: changed }), "Done · not merged");
  assert.equal(word({ working: false, lastEnd: "done" }), "Done");
  assert.equal(word({ working: false, lastEnd: "done", gone: true }), "Done");
});

test("the finished ones count as N done · M stopped, failed among the stopped", () => {
  const rows = [
    row({ working: false, lastEnd: "done" }),
    row({ working: false, lastEnd: "done", merged: true }),
    row({ working: false, lastEnd: "done", ahead: 1 }),
    row({ working: false, lastEnd: "stopped" }),
    row({ working: false, lastEnd: "failed" }),
    row(),
  ];
  assert.deepEqual(finishedCounts(rows, NOW), { done: 3, stopped: 2 });
  assert.equal(finishedWords(rows, NOW), "3 done · 2 stopped");
  assert.equal(finishedWords([row()], NOW), "");
});

test("rows are named by their titles, unique in the session in the order they came; unmerged ones are listed for archiving", () => {
  const names = rowNames([row({ id: "b", number: 2, title: "Fix the login page" }), row({ id: "a", number: 1, title: "Fix the login page" })]);
  assert.equal(names.get("a"), "Fix the login page");
  assert.equal(names.get("b"), "Fix the login 2");
  const unmerged = unmergedRows([row({ id: "x", ahead: 1 }), row({ id: "y", ahead: 1, merged: true }), row({ id: "z", diff: changed }), row({ id: "w" })]);
  assert.deepEqual(unmerged.map((each) => each.id), ["x", "z"]);
});
