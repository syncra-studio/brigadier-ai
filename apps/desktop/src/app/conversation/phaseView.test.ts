import assert from "node:assert/strict";
import { test } from "node:test";

import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import {
  phaseMark,
  phaseOutcome,
  phaseViewOf,
  phaseWord,
  runPill,
  runSteps,
  splitReport,
  type RunStep,
} from "@/app/conversation/phaseView";
import { formatDuration } from "@/lib/format";
import type { OvernightRun, Plan, PlanStep } from "@/ipc/generated";

const runs = night.overnight as unknown as Record<string, OvernightRun>;
const run = Object.values(runs)[0] as OvernightRun;
const plans = night.plans as unknown as Record<string, Plan>;
const plan = plans[run.planId ?? ""] as Plan;
const RUN_REQUEST = "run-09cc7d53-g1";

/** A step's state as the report heads it: "✓ done", "not reached". */
function said(step: RunStep, over: boolean): string {
  const mark = phaseMark(step.mark, over);
  return mark ? `${mark} ${phaseWord(step.mark, over)}` : phaseWord(step.mark, over);
}

/** `step` as it was before the thread settled it. */
function unsettled(step: PlanStep): PlanStep {
  const copy = { ...step };
  delete copy.settled;
  return copy;
}

/** The night's run with its plan's steps changed by `change`, in state `state`. */
function withSteps(
  state: OvernightRun["state"],
  change: (step: PlanStep, index: number) => PlanStep,
  extra: Partial<OvernightRun> = {},
): { runs: Record<string, OvernightRun>; plans: Record<string, Plan> } {
  const live = { ...run, state, finishedAtMs: state === "finished" ? run.finishedAtMs : null, ...extra };
  return {
    runs: { [run.id]: live },
    plans: { ...plans, [plan.id]: { ...plan, steps: plan.steps.map(change) } },
  };
}

test("the night's phases read as its report heads them: done, partial, not reached", () => {
  const steps = runSteps(run, plans);
  assert.deepEqual(
    steps.map((step) => `Phase ${step.number} · ${step.name}: ${said(step, true)}`),
    ["Phase 1 · Measure: ✓ done", "Phase 2 · Fix: ◐ partial", "Phase 3 · Re-measure: not reached"],
  );
  // Done reads its settlement's first sentence; partial what is left of it, said to the user.
  assert.deepEqual(
    steps.map((step) => phaseOutcome(run, step, true)),
    [
      "The findings note has the per-role table and the top 3 causes ranked by minutes lost, each with a fix.",
      "The findings note's phase 2 section isn't on the run branch: you stopped the run first.",
      "Not reached: the run ended first.",
    ],
  );
  // Phase 1 is timed from its start to when the thread settled it.
  const [measure] = steps;
  assert.equal(formatDuration((measure?.endedAtMs ?? 0) - (measure?.startedAtMs ?? 0)), "2h 31m");
});

test("a done phase without a summary says the tip it was done at", () => {
  const [measure] = runSteps(run, plans);
  assert.ok(measure?.settled);
  const bare = { ...measure, settled: { ...measure.settled, summary: "" } };
  assert.equal(phaseOutcome(run, bare, true), "Done at 1dcda64.");
});

test("a blocked phase says what only the user can do", () => {
  const blocked = withSteps("finished", (step, index) =>
    index === 2
      ? {
          ...step,
          stage: "building",
          settled: { outcome: "blocked", summary: "Stuck.", left: ["The re-run needs the user to sign in to Codex."], tip: null, atMs: 1 },
        }
      : step,
  );
  const step = runSteps(blocked.runs[run.id]!, blocked.plans)[2]!;
  assert.equal(said(step, true), "✕ blocked");
  assert.equal(phaseOutcome(run, step, true), "The re-run needs you to sign in to Codex.");
});

test("a phase the run's restrictions left out is skipped, before and after the run", () => {
  const skipped = withSteps("running", (step, index) => (index === 2 ? { ...step, stage: "skipped" } : step));
  const step = runSteps(skipped.runs[run.id]!, skipped.plans)[2]!;
  assert.equal(said(step, false), "– skipped");
  assert.equal(said(step, true), "– skipped");
  assert.equal(phaseOutcome(run, step, true), "Skipped, as the run's restrictions said.");
  // Before the run has a plan, its restrictions say which phases it skips.
  const proposed: OvernightRun = { ...run, planId: null, state: "proposed", directives: { ...run.directives, skip: [2] } };
  assert.deepEqual(
    runSteps(proposed, {}).map((one) => said(one, false)),
    ["not started", "– skipped", "not started"],
  );
});

test("a phase not begun is not reached once the run is over; one begun and not settled is unfinished", () => {
  const cut = withSteps("finished", (step, index) =>
    index === 1 ? unsettled(step) : step,
  );
  const steps = runSteps(cut.runs[run.id]!, cut.plans);
  assert.equal(said(steps[1]!, true), "◐ unfinished");
  assert.equal(phaseOutcome(run, steps[1]!, true), "The run ended during this phase (stopped by you).");
  assert.equal(said(steps[2]!, true), "not reached");
  // While the run works they are at work and not started, with no outcome yet.
  assert.equal(said(steps[1]!, false), "working");
  assert.equal(said(steps[2]!, false), "not started");
  assert.equal(phaseOutcome(run, steps[1]!, false), null);
  assert.equal(phaseOutcome(run, steps[2]!, false), null);
});

test("the run's one request is one block: how many phases were done once it is over", () => {
  const view = phaseViewOf(runs, plans, RUN_REQUEST);
  assert.ok(view);
  assert.equal(view.title, "Overnight · Faster, leaner overnight runs");
  assert.equal(`${view.mark} ${view.word}`, "◐ 1 of 3 phases done");
  assert.equal(view.settled, true);
  assert.equal(view.outcome, "1 of 3 phases done.");
  assert.equal(view.endedAtMs, run.finishedAtMs);
  // Not a run's request, or its report's: no run block.
  assert.equal(phaseViewOf(runs, plans, "run-09cc7d53-report"), null);
  assert.equal(phaseViewOf(runs, plans, "01a0fefa-e29d-74c7-ac52-3d35081523da"), null);
});

test("a run's block while it works says the phase at work, and is live", () => {
  const live = withSteps("running", (step, index) =>
    index === 1 ? unsettled(step) : index === 2 ? { ...step, startedAtMs: null } : step,
  );
  const view = phaseViewOf(live.runs, live.plans, RUN_REQUEST);
  assert.ok(view);
  assert.equal(view.settled, false);
  assert.equal(view.word, "Phase 2 of 3 · Fix");
  assert.equal(view.endedAtMs, null);
  assert.equal(view.outcome, null);
  // Between phases: the first one not begun.
  const between = withSteps("running", (step, index) => (index === 1 ? { ...unsettled(step), stage: "pending" } : step));
  assert.equal(phaseViewOf(between.runs, between.plans, RUN_REQUEST)?.word, "Phase 2 of 3 · Fix");
});

test("a run whose every phase it worked on is done is marked done, skipped ones aside", () => {
  const done = withSteps("finished", (step, index) =>
    index === 2 ? { ...step, stage: "skipped" } : { ...step, settled: { ...step.settled!, outcome: "done" } },
  );
  const view = phaseViewOf(done.runs, done.plans, RUN_REQUEST);
  assert.equal(view && `${view.mark} ${view.word}`, "✓ 2 of 2 phases done");
});

test("during a run the pill shows the phase at work and the run's phases", () => {
  const live = withSteps("running", (step, index) => (index === 1 ? unsettled(step) : step));
  const pill = runPill(live.runs, live.plans);
  assert.ok(pill);
  assert.equal(pill.label, "Phase 2 of 3 · Fix");
  assert.deepEqual(
    pill.steps.map((step) => [step.title, step.done, step.active]),
    [
      ["Measure", true, false],
      ["Fix", false, true],
      ["Re-measure", false, false],
    ],
  );
  assert.equal(`${pill.done}/${pill.total}`, "1/3");
});

test("a pill names the plan's own number, without 'of m' when it isn't the phase's place", () => {
  // The user ran only phases 4 to 6 of their plan: the run's plan keeps their numbers.
  const later = withSteps("running", (step, index) => ({
    ...(index === 0 ? step : unsettled(step)),
    number: index + 4,
    stage: index === 1 ? "building" : index === 2 ? "pending" : step.stage,
  }));
  assert.equal(runPill(later.runs, later.plans)?.label, "Phase 5 · Fix");
  // Writing the report: the pill says so, and lists nothing.
  const ending = withSteps("windingDown", (step) => step);
  assert.deepEqual(runPill(ending.runs, ending.plans), {
    runId: run.id,
    label: "Writing your report",
    done: 0,
    total: 0,
    steps: [],
  });
});

test("once the run is over the pill shows nothing of it", () => {
  assert.equal(runPill(runs, plans), null);
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
