import assert from "node:assert/strict";
import { test } from "node:test";

import type { BoardJob } from "@/app/conversation/boardJob";
import type { OvernightCardModel } from "@/app/conversation/overnightAdapter";
import { runInSlot } from "@/app/conversation/runJob";
import type { OvernightRun } from "@/ipc/generated";

const model = (id: string, change: Partial<OvernightRun>): OvernightCardModel => ({
  run: { id, state: "proposed", merged: null, finishedAtMs: null, ...change } as OvernightRun,
  details: { waiting: 0, decided: 0, steps: [], phaseProgress: {}, remaining: 0 },
});
const job = (atMs: number) => ({ steps: [], atMs, facts: { findings: [], merge: null } }) as BoardJob;

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
