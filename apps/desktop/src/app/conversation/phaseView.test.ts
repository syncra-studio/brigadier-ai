import assert from "node:assert/strict";
import { test } from "node:test";

import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import { phaseViewOf, phaseWord } from "@/app/conversation/phaseView";
import { formatDuration } from "@/lib/format";
import type { OvernightRun } from "@/ipc/generated";

const runs = night.overnight as unknown as Record<string, OvernightRun>;
const run = Object.values(runs)[0] as OvernightRun;

test("a verified phase folds to what it met, timed from its start to when it settled", () => {
  const phase = phaseViewOf(runs, "run-09cc7d53-phase-1-g1");
  assert.ok(phase);
  assert.equal(phase.title, "Phase 1 · Measure");
  assert.equal(`${phase.mark} ${phase.word}`, "✓ verified");
  assert.equal(phase.settled, true);
  assert.equal(formatDuration((phase.endedAtMs ?? 0) - (phase.startedAtMs ?? 0)), "2h 31m");
  assert.equal(phase.outcome, "1 of 1 done-when criterion met · verified at 1dcda64.");
});

test("a partial phase stays settled though something still waits on the user, and says its gap", () => {
  // The Phase 2 request is still "waiting" on an open item; the phase settled at the Stop.
  assert.equal(night.requests["run-09cc7d53-phase-2-g1"].state.type, "waiting");
  const phase = phaseViewOf(runs, "run-09cc7d53-phase-2-g1");
  assert.ok(phase);
  assert.equal(`${phase.mark} ${phase.word}`, "◐ partial");
  assert.equal(phase.settled, true);
  assert.equal(formatDuration((phase.endedAtMs ?? 0) - (phase.startedAtMs ?? 0)), "1h 53m");
  assert.equal(phase.outcome, "Its whole-phase checks never passed: the user stopped the run.");
});

test("a phase the run never reached reads the same on the card as in the report", () => {
  assert.equal(phaseWord("pending", true), "not reached");
  assert.equal(phaseWord("pending", false), "not started");
  assert.equal(run.phases[2]?.state, "pending");
});

test("a phase still at work is live and has no outcome yet", () => {
  const live: Record<string, OvernightRun> = {
    [run.id]: {
      ...run,
      state: "running",
      finishedAtMs: null,
      phases: run.phases.map((phase) =>
        phase.id === "phase-2" ? { ...phase, state: "running", settledAtMs: null } : phase,
      ),
    },
  };
  const phase = phaseViewOf(live, "run-09cc7d53-phase-2-g1");
  assert.ok(phase);
  assert.equal(phase.settled, false);
  assert.equal(phase.word, "working");
  assert.equal(phase.endedAtMs, null);
  assert.equal(phase.outcome, null);
});
