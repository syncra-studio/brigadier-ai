import assert from "node:assert/strict";
import { test } from "node:test";

import { headerLabel, workersWaitWords } from "@/app/conversation/requestTime";
import { workingWorkers } from "@/app/conversation/turnWorkers";
import type { WorkerRow } from "@/ipc/generated";

const worker = (id: string, startedAtMs: number, more: Partial<WorkerRow> = {}): WorkerRow => ({
  id,
  number: 1,
  title: id,
  agent: "claude",
  model: null,
  startedAtMs,
  working: true,
  lastEnd: null,
  gone: false,
  lastEventAtMs: null,
  limit: null,
  freshContext: false,
  waiting: [],
  mcp: [],
  branch: null,
  base: null,
  diff: null,
  ahead: 0,
  merged: false,
  ...more,
});

const turns = [{ startedAtMs: 1000 }, { startedAtMs: 5000 }];

test("a turn holds for the workers it started until each finished", () => {
  // Started in the first turn, still on its first run: the first turn holds, the second doesn't.
  assert.equal(workingWorkers([worker("w1", 1500)], turns, [1000]), 1);
  assert.equal(workingWorkers([worker("w1", 1500)], turns, [5000]), 0);
  // Started after the newest turn's start (a turn the CLI began on its own after a wait ended is the same turn).
  assert.equal(workingWorkers([worker("w2", 9000)], turns, [5000]), 1);
  // Not started yet by its CLI: not finished either.
  assert.equal(workingWorkers([worker("w3", 6000, { working: false })], turns, [5000]), 1);
  // Finished, or gone: no hold.
  assert.equal(workingWorkers([worker("w4", 6000, { working: false, lastEnd: "done" })], turns, [5000]), 0);
  assert.equal(workingWorkers([worker("w5", 6000, { working: false, lastEnd: "failed", gone: true })], turns, [5000]), 0);
  assert.equal(workingWorkers([worker("w6", 6000, { gone: true })], turns, [5000]), 0);
});

test("a worker waiting on the user still holds its turn; sent back to work, only the newest turn's", () => {
  const asks = worker("w1", 6000, { working: false, lastEnd: "done", waiting: [{ kind: "question", what: "Which?", card: null }] });
  assert.equal(workingWorkers([asks], turns, [5000]), 1);
  const again = worker("w2", 1500, { working: true, lastEnd: "done" });
  // A later turn sent it more work: the older turn it started in stays folded.
  assert.equal(workingWorkers([again], turns, [1000]), 0);
  assert.equal(workingWorkers([{ ...again, startedAtMs: 6000 }], turns, [5000]), 1);
});

test("a block Brigadier's word of its workers joined holds for the workers any of its turns started", () => {
  const three = [{ startedAtMs: 1000 }, { startedAtMs: 5000 }, { startedAtMs: 8000 }];
  // The block is the first two turns: a worker its joined turn started counts, the third turn's doesn't.
  assert.equal(workingWorkers([worker("w1", 6000)], three, [1000, 5000]), 1);
  assert.equal(workingWorkers([worker("w2", 8500)], three, [1000, 5000]), 0);
  assert.equal(workingWorkers([worker("w1", 6000)], three, []), 0);
});

test("a turn held by its workers says it waits for them", () => {
  assert.equal(workersWaitWords(1), "Waiting for a worker");
  assert.equal(workersWaitWords(3), "Waiting for 3 workers");
  assert.equal(headerLabel("done", 65_000, false, false, 2), "Waiting for 2 workers · 1m 5s");
  assert.equal(headerLabel("done", 65_000), "Worked for 1m 5s");
});

test("a block its later turns joined holds for the workers each of them started; only a turn of a block after it bounds it", () => {
  const joinedTurns = [{ startedAtMs: 1000 }, { startedAtMs: 3000 }, { startedAtMs: 5000 }, { startedAtMs: 9000 }];
  const workers = [worker("w1", 1500), worker("w2", 3500), worker("w3", 9500)];
  // The turns at 3000 and 5000 joined the block of the turn at 1000: its workers are w1 and w2.
  assert.equal(workingWorkers(workers, joinedTurns, [1000, 3000, 5000]), 2);
  // Unjoined, the turn at 3000 would bound it to w1.
  assert.equal(workingWorkers(workers, joinedTurns, [1000]), 1);
});

test("a worker whose turn ended while background commands of its own still run holds its block until they end", () => {
  // Its row reads at work (the engine counts its running background commands): the block, its
  // later turn joined, still waits for it.
  assert.equal(workingWorkers([worker("w1", 1500, { lastEnd: "done", working: true })], turns, [1000, 5000]), 1);
  // They ended: it is done.
  assert.equal(workingWorkers([worker("w1", 1500, { lastEnd: "done", working: false })], turns, [1000, 5000]), 0);
});

test("the session CLI's own subagents never hold a turn nor count among the workers it waits for", () => {
  const subagent = { ...worker("native:c:call1", 1500), native: { conversationId: "c", callId: "call1", backgroundId: "b1", prompt: "" } };
  assert.equal(workingWorkers([subagent], turns, [1000, 5000]), 0);
  assert.equal(workingWorkers([subagent, worker("w1", 1500)], turns, [1000, 5000]), 1);
});
