import assert from "node:assert/strict";
import { test } from "node:test";

import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import { phaseViewOf, phaseWord, runPill, splitReport } from "@/app/conversation/phaseView";
import { formatDuration } from "@/lib/format";
import type { OvernightRun, Plan, Task } from "@/ipc/generated";

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

/** The night's run as it was while `phaseId` was at work. */
function liveAt(phaseId: string): Record<string, OvernightRun> {
  return {
    [run.id]: {
      ...run,
      state: "running",
      finishedAtMs: null,
      phases: run.phases.map((phase) =>
        phase.id === phaseId
          ? { ...phase, state: "running", settledAtMs: null }
          : phase.id > phaseId
            ? { ...phase, state: "pending" }
            : phase,
      ),
    },
  };
}

const plans = night.plans as unknown as Record<string, Plan>;
const tasks = night.tasks as unknown as Record<string, Task>;

test("during a run the pill shows the run's phase and the steps of its approved plan", () => {
  const pill = runPill(liveAt("phase-2"), plans, tasks);
  assert.ok(pill);
  assert.equal(pill.label, "Phase 2 of 3 · Fix · 3 of 4 steps");
  assert.deepEqual(
    pill.steps.map((step) => step.done),
    [true, true, false, true],
  );
  // A phase without a plan of its own says only where the run is.
  assert.equal(runPill(liveAt("phase-1"), plans, tasks)?.label, "Phase 1 of 3 · Measure");
});

test("once the run is over the pill shows nothing of it", () => {
  assert.equal(runPill(runs, plans, tasks), null);
});

test("a report shows down to its Details heading and folds the rest", () => {
  const report = "**Run** finished.\n\n### What got in the way\n- Held task-3\n\n### Details\n#### Workers and models\n- task-1 · opus\n";
  assert.deepEqual(splitReport(report), {
    head: "**Run** finished.\n\n### What got in the way\n- Held task-3",
    details: "#### Workers and models\n- task-1 · opus",
  });
  // Only the heading itself folds: a line that merely mentions it doesn't, nor an empty fold.
  assert.equal(splitReport("See ### Details below.\n"), null);
  assert.equal(splitReport("Done.\n\n### Details\n"), null);
  assert.equal(splitReport("Done.\n\n### Details"), null);
});
