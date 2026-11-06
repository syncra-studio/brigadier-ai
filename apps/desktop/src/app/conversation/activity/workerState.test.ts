import assert from "node:assert/strict";
import { test } from "node:test";

import { endedWord, finishedCounts, finishedWords, QUIET_MS, rowNames, rowOnHand, workerStateOf } from "@/app/conversation/activity/workerState";
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

test("a kept worker whose CLI ended says so, stays on hand and is no failure", () => {
  const ended = row({ working: false, lastEnd: "interrupted", cliEnded: "Its CLI ended; the next message continues it (signal 15)." });
  assert.equal(workerStateOf(ended, NOW).word, "Its CLI ended · next message continues it");
  assert.equal(rowOnHand(ended, NOW), true);
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

test("what holds its work up reads on its row, by the real minutes", () => {
  const hold = (patch: object) => ({ jobHold: { kind: "stalled", sinceMs: NOW - 12 * 60_000, what: null, load: null, cores: null, ...patch } as WorkerRow["jobHold"] });
  assert.equal(word(hold({ what: "cargo test" })), "Stalled 12m · cargo test");
  assert.equal(word(hold({ kind: "buildSlot" })), "Waiting for a build slot 12m");
  assert.equal(word(hold({ kind: "starved", load: 60, cores: 14 })), "Build starved 12m · load 60 on 14 cores");
  // Its turn is over and only its commands run: never quiet, whatever its CLI last said.
  assert.equal(word({ onlyJobs: true, lastEventAtMs: NOW - 2 * QUIET_MS }), "Working");
  // Ended: what held it up is over.
  assert.equal(word({ working: false, lastEnd: "done", ...hold({}) }), "Done");
});

test("how its last turn ended: failed (or cut off, or gone without a done end), stopped", () => {
  assert.equal(word({ working: false, lastEnd: "failed" }), "Failed");
  assert.equal(word({ working: false, lastEnd: "interrupted" }), "Failed");
  assert.equal(word({ working: false, lastEnd: null, gone: true }), "Failed");
  assert.equal(word({ working: false, lastEnd: "stopped" }), "Ended");
  assert.equal(word({ working: false, lastEnd: "stopped", gone: true, merged: true }), "Ended");
});

test("a worker its session paused reads Paused, never Failed, its cut turn whatever it was (O57 item 3)", () => {
  assert.equal(word({ working: false, lastEnd: "interrupted", paused: true }), "Paused");
  assert.equal(word({ working: false, lastEnd: null, paused: true }), "Paused");
});

test("git says the rest: Merged, Done · not merged (commits ahead or changes), Done", () => {
  assert.equal(word({ working: false, lastEnd: "done", merged: true, ahead: 0 }), "Merged");
  assert.equal(word({ working: false, lastEnd: "done", ahead: 2 }), "Done · not merged");
  assert.equal(word({ working: false, lastEnd: "done", diff: changed }), "Done · not merged");
  assert.equal(word({ working: false, lastEnd: "done" }), "Done");
  assert.equal(word({ working: false, lastEnd: "done", gone: true }), "Done");
});

test("the finished ones count as N done · M ended, failed among the ended, merged among the done; a paused one is on hand (D198, D199)", () => {
  const rows = [
    row({ working: false, lastEnd: "done" }),
    row({ working: false, lastEnd: "done", merged: true }),
    row({ working: false, lastEnd: "done", ahead: 1 }),
    row({ working: false, lastEnd: "stopped" }),
    row({ working: false, lastEnd: "failed" }),
    row(),
  ];
  assert.deepEqual(finishedCounts(rows, NOW), { done: 3, ended: 2 });
  assert.equal(finishedWords(rows, NOW), "3 done · 2 ended");
  const paused = row({ working: false, lastEnd: "interrupted", paused: true });
  assert.equal(finishedWords([...rows, paused], NOW), "3 done · 2 ended");
  assert.equal(rowOnHand(paused, NOW), true);
  assert.equal(rowOnHand(rows[3]!, NOW), false);
  assert.equal(endedWord(workerStateOf(rows[1]!, NOW).kind), "Done");
  assert.equal(finishedWords([row()], NOW), "");
});

test("rows are named by their titles, unique in the session in the order they came", () => {
  const names = rowNames([row({ id: "b", number: 2, title: "Fix the login page" }), row({ id: "a", number: 1, title: "Fix the login page" })]);
  assert.equal(names.get("a"), "Fix the login page");
  assert.equal(names.get("b"), "Fix the login 2");
});
