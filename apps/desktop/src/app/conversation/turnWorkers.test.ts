import assert from "node:assert/strict";
import { test } from "node:test";

import { headerLabel, workersWaitWords } from "@/app/conversation/requestTime";
import { reopened, workingWorkers } from "@/app/conversation/turnWorkers";
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
  // Started in the first turn, still on its first run: the first turn holds.
  assert.equal(workingWorkers([worker("w1", 1500)], turns, [1000]), 1);
  // Started after the newest turn's start (a turn the CLI began on its own after a wait ended is the same turn).
  assert.equal(workingWorkers([worker("w2", 9000)], turns, [5000]), 1);
  // Not started yet by its CLI: not finished either.
  assert.equal(workingWorkers([worker("w3", 6000, { working: false })], turns, [5000]), 1);
  // Finished, or gone: no hold.
  assert.equal(workingWorkers([worker("w4", 6000, { working: false, lastEnd: "done" })], turns, [5000]), 0);
  assert.equal(workingWorkers([worker("w5", 6000, { working: false, lastEnd: "failed", gone: true })], turns, [5000]), 0);
  assert.equal(workingWorkers([worker("w6", 6000, { gone: true })], turns, [5000]), 0);
});

test("a worker an earlier block started that the newest block resumed or switched holds that block until it ends (D24, D151)", () => {
  // Started at 1500 in the first block, paused, then sent back to work in the newest one (5000).
  const resumed = worker("w1", 1500, { lastEnd: "stopped" });
  assert.equal(workingWorkers([resumed], turns, [5000], 6000), 1, "its turn ended, the worker still works: Working for");
  assert.equal(workingWorkers([resumed], turns, [1000], 4000), 0, "the older block stays closed");
  assert.equal(workingWorkers([{ ...resumed, working: false, lastEnd: "done" }], turns, [5000], 6000), 0, "ended: it closes");
});

test("a paused worker keeps its block open until it resumes or stops; stopped, it holds nothing (D159)", () => {
  // Paused in the block that started it: its turn was cut ("interrupted"), it runs nothing.
  const paused = worker("w2", 5500, { working: false, paused: true, lastEnd: "interrupted" });
  assert.equal(workingWorkers([paused], turns, [5000], 6000), 1, "paused: the block stays open, Working for");
  // One an earlier block started, paused by the newest block: the newest holds, the older stays closed.
  const older = worker("w1", 1500, { working: false, paused: true, lastEnd: "interrupted" });
  assert.equal(workingWorkers([older], turns, [5000], 6000), 1);
  assert.equal(workingWorkers([older], turns, [1000], 4000), 0);
  // Stopped: gone, it no longer holds the block.
  assert.equal(workingWorkers([{ ...paused, paused: false, gone: true, lastEnd: "stopped" }], turns, [5000], 6000), 0);
});

test("a worker that ended after its block's turns holds the block until the CLI's turn for its word starts (D24, D94; seen live on Codex)", () => {
  // Its block's turns ended at 7000; it ended at 9000 and its word hasn't reached the CLI yet.
  const ended = worker("w1", 6000, { working: false, lastEnd: "done", lastEventAtMs: 9000 });
  assert.equal(workingWorkers([ended], turns, [5000], 7000, true), 1);
  assert.equal(workingWorkers([ended], turns, [5000], 7000, false), 0, "its turn began, or the grace passed");
  // One whose word a turn of the block already took (the block ends after it) holds nothing.
  assert.equal(workingWorkers([ended], turns, [5000], 9500, true), 0);
  // Nor in an older block.
  assert.equal(workingWorkers([{ ...ended, startedAtMs: 1500 }], turns, [1000], 4000, true), 0);
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

test("a turn its background job's end woke is open again for its block, a later turn never is (D24)", () => {
  // The turn ran 1000–5000; the subagent's end reopens it, its start unchanged.
  assert.equal(reopened(1000, 1000, 5000), true);
  // A block whose message was shown before the CLI said its turn's start.
  assert.equal(reopened(1200, 1000, 5000), true);
  // The user's next turn, the thread's newest turn closed, a block still running.
  assert.equal(reopened(6000, 1000, 5000), false);
  assert.equal(reopened(null, 1000, 5000), false);
  assert.equal(reopened(1000, 1000, null), false);
});
