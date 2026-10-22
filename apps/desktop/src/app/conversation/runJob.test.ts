import assert from "node:assert/strict";
import { test } from "node:test";

import { type BoardJob, sessionJobs } from "@/app/conversation/boardJob";
import type { OvernightCardModel } from "@/app/conversation/overnightAdapter";
import { runInSlot } from "@/app/conversation/runJob";
import type { OvernightRun, Task } from "@/ipc/generated";
import { emptyBoard } from "@/state/board";

const model = (id: string, change: Partial<OvernightRun>): OvernightCardModel => ({
  run: { id, state: "proposed", merged: null, finishedAtMs: null, ...change } as OvernightRun,
  details: { waiting: 0, decided: 0, steps: [], phaseProgress: {}, remaining: 0 },
});
const job = (atMs: number) => ({ steps: [], atMs, latestStartedAtMs: atMs }) as BoardJob;
const task = (id: string, createdAtMs: number, state: Task["state"]): Task =>
  ({ id, kind: "implement", requestId: "r1", createdAtMs, updatedAtMs: 200, state }) as Task;

test("the newest run holds the Job card while proposed, at work, and over until merged or new work starts", () => {
  assert.equal(runInSlot([], job(10)), null);
  // Proposed or at work: the run's, whatever the session does beside it.
  for (const state of ["proposed", "preparing", "running", "waitingQuota", "windingDown", "reporting"] as const) {
    assert.equal(runInSlot([model("a", { state })], job(10))?.run.id, "a", state);
  }
  // Over: the run's until its work is merged.
  const over = model("a", { state: "finished", finishedAtMs: 100 });
  assert.equal(runInSlot([over], null)?.run.id, "a");
  assert.equal(runInSlot([over], job(50))?.run.id, "a");
  assert.equal(runInSlot([model("a", { state: "finished", finishedAtMs: 100, merged: { verifiedCommit: "x", commit: "y", into: "main", atMs: 120 } })], job(50)), null);
  // Work the session started after the run ended has the card back.
  assert.equal(runInSlot([over], job(150)), null);
  // The newest run that is not replaced by its continuation.
  assert.equal(runInSlot([model("a", { state: "finished", finishedAtMs: 100 }), model("b", { state: "running" })], null)?.run.id, "b");
  assert.equal(runInSlot([model("a", { state: "running" }), model("b", { state: "superseded" })], null)?.run.id, "a");
});

test("new work after a run takes the card even when unmerged pre-run work shares its Building step", () => {
  const over = model("a", { state: "finished", finishedAtMs: 100 });
  const input = {
    ...emptyBoard("c"),
    tasks: { old: task("old", 10, "landed"), newer: task("newer", 100, "running") },
  };
  // Updated work and work started exactly when the run ended do not replace it.
  assert.equal(runInSlot([over], sessionJobs(input).current), over);
  input.tasks.newer = task("newer", 150, "running");
  const current = sessionJobs(input).current!;
  assert.equal(current.atMs, 10);
  assert.equal(current.steps[0]?.startedAtMs, 10);
  assert.equal(runInSlot([over], current), null);
});
