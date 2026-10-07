import type { OvernightRun, PhaseStage, Plan, StepSettlement } from "@/ipc/generated";

/**
 * An overnight run's phases as everything that names them says them: the run card, the run's
 * block in the thread and the composer's pill use the same marks and words (and so does the
 * morning report, from the daemon's `report.rs`).
 */

/**
 * A morning report split at its `### Details` heading: what shows, and the details the thread
 * folds (the whole-phase checks, workers and models, provider usage). Null when it has none.
 */
export function splitReport(text: string): { head: string; details: string } | null {
  const at = text.search(/^### Details[ \t]*$/m);
  const end = at < 0 ? -1 : text.indexOf("\n", at);
  if (end < 0) return null;
  const details = text.slice(end + 1).trim();
  return details ? { head: text.slice(0, at).trimEnd(), details } : null;
}

/** Whether the run is over: nothing more happens in any of its phases. */
export function runOver(run: OvernightRun): boolean {
  return run.state === "finished" || run.state === "superseded";
}

/** A run at work: started and not over yet. */
export function runActive(run: OvernightRun): boolean {
  return run.state !== "proposed" && !runOver(run);
}

/** The run's one request in the thread (the daemon's `run_request`): every phase's work is in it. */
export function runRequestId(run: OvernightRun): string {
  return `run-${run.id.slice(-8)}-g${run.generation}`;
}

/**
 * The run's plan (the daemon's `run_plan`): the one Start recorded, else the newest approved
 * plan of the run's own request (one the thread made for a bare goal).
 */
export function runPlan(run: OvernightRun, plans: Readonly<Record<string, Plan>>): Plan | null {
  const recorded = run.planId ? plans[run.planId] : undefined;
  if (recorded) return recorded;
  const prefix = `run-${run.id.slice(-8)}-`;
  return (
    Object.values(plans)
      .filter((plan) => plan.requestId?.startsWith(prefix) && plan.state.type === "approved")
      .toSorted((a, b) => b.createdAtMs - a.createdAtMs)[0] ?? null
  );
}

/**
 * Where a phase stands: settled by the thread (done, partial, blocked), left out by the run's
 * restrictions (skipped), not begun (pending) or begun and not settled (working).
 */
export type StepMark = StepSettlement["outcome"] | "skipped" | "pending" | "working";

/** A phase of a run as its plan step records it (the daemon's `steps`, in `report.rs`). */
export type RunStep = {
  /** The source plan's own number. */
  number: number;
  /** Its place in the run's plan, from 1. */
  position: number;
  name: string;
  mark: StepMark;
  settled: StepSettlement | null;
  startedAtMs: number | null;
  /** When the thread settled it, else when its work ended. */
  endedAtMs: number | null;
};

function stepMark(stage: PhaseStage, settled: StepSettlement | undefined): StepMark {
  if (settled) return settled.outcome;
  if (stage === "skipped" || stage === "pending") return stage;
  return "working";
}

/** Whether the run's restrictions take in phase `number` (the daemon's `selects`). */
function selects(run: OvernightRun, number: number): boolean {
  const only = run.directives.only;
  return (!only || (number >= only.from && number <= only.to)) && !run.directives.skip.includes(number);
}

/** The run's phases from its plan; before it has one, its phases as Start agreed them. */
export function runSteps(run: OvernightRun, plans: Readonly<Record<string, Plan>>): RunStep[] {
  const plan = runPlan(run, plans);
  if (plan) {
    return plan.steps.map((step, index) => ({
      number: step.number ?? index + 1,
      position: index + 1,
      name: step.title,
      mark: stepMark(step.stage, step.settled),
      settled: step.settled ?? null,
      startedAtMs: step.startedAtMs,
      endedAtMs: step.settled?.atMs ?? step.endedAtMs,
    }));
  }
  return run.phases.map((phase, index) => ({
    number: phase.number,
    position: index + 1,
    name: phase.name,
    mark: selects(run, phase.number) ? "pending" : "skipped",
    settled: null,
    startedAtMs: null,
    endedAtMs: null,
  }));
}

/** The mark beside a phase's state, as the report heads it: what it came to, never colour alone. */
export function phaseMark(mark: StepMark, over: boolean): string {
  switch (mark) {
    case "done":
      return "✓";
    case "partial":
      return "◐";
    case "blocked":
      return "✕";
    case "skipped":
      return "–";
    case "pending":
      return "";
    case "working":
      return over ? "◐" : "";
  }
}

/** A phase's state in a word or two (the report's `phase_word`); `over` once the run has ended. */
export function phaseWord(mark: StepMark, over: boolean): string {
  switch (mark) {
    case "pending":
      return over ? "not reached" : "not started";
    case "working":
      return over ? "unfinished" : "working";
    default:
      return mark;
  }
}

/**
 * The run's stored words about the user, as said to the user (the daemon's `TO_YOU`, in
 * `wind_down.rs`): what is left of a phase keeps them for the thread.
 */
const TO_YOU: ReadonlyArray<readonly [string, string]> = [
  ["the user stopped the run", "you stopped the run"],
  ["the run stopped as the user asked", "the run stopped as you asked"],
  [" needs the user", " needs you"],
];

/** `text` with the run's words about the user said to the user. */
export function toYou(text: string): string {
  return TO_YOU.reduce((said, [stored, shown]) => said.replaceAll(stored, shown), text);
}

/** The first sentence of a text's first line. */
function firstSentence(text: string): string {
  const line = text.trim().split("\n")[0] ?? "";
  const end = line.search(/[.!?](\s|$)/);
  return end < 0 ? line : line.slice(0, end + 1);
}

/** What came of a phase in a line, as the run card says it; null while it works. */
export function phaseOutcome(run: OvernightRun, step: RunStep, over: boolean): string | null {
  switch (step.mark) {
    case "done": {
      const summary = firstSentence(step.settled?.summary ?? "");
      if (summary) return summary;
      const tip = step.settled?.tip;
      return tip ? `Done at ${tip.slice(0, 7)}.` : "Done.";
    }
    case "partial":
    case "blocked":
      return toYou(step.settled?.left[0] ?? step.settled?.summary ?? "");
    case "skipped":
      return "Skipped, as the run's restrictions said.";
    case "pending":
      return over ? "Not reached: the run ended first." : null;
    case "working":
      return over
        ? `The run ended during this phase (${run.stop?.type === "stopped" ? "stopped by you" : "its time was up"}).`
        : null;
  }
}

/** "Phase 2 of 3 · Fix": "of m" only when the plan numbers its phases from 1 in order. */
export function phaseTitle(step: RunStep, total: number): string {
  return `Phase ${step.number === step.position ? `${step.number} of ${total}` : step.number} · ${step.name}`;
}

/**
 * The phase the run works on: the first it began and hasn't settled, else the first it hasn't
 * begun. Null once nothing is left to work on.
 */
export function currentStep(steps: readonly RunStep[]): RunStep | null {
  return steps.find((step) => step.mark === "working") ?? steps.find((step) => step.mark === "pending") ?? null;
}

/** The phases settled done, and those worked on: phases the run left out don't count. */
export function phasesDone(steps: readonly RunStep[]): { done: number; worked: number } {
  const worked = steps.filter((step) => step.mark !== "skipped").length;
  return { done: steps.filter((step) => step.mark === "done").length, worked };
}

/** The run's block in the thread: its header and, once over, the one line it folds to. */
export type PhaseView = {
  title: string;
  mark: string;
  word: string;
  settled: boolean;
  startedAtMs: number | null;
  endedAtMs: number | null;
  /** What it came to, in a sentence, once over. */
  outcome: string | null;
};

/** The run whose request `requestId` is, in any of its generations. */
function runOf(runs: Readonly<Record<string, OvernightRun>>, requestId: string): OvernightRun | null {
  return Object.values(runs).find((run) => requestId.startsWith(`run-${run.id.slice(-8)}-g`)) ?? null;
}

/**
 * The block of an overnight run's request: the phase at work while the run works, how many of
 * its phases were done once it is over.
 */
export function phaseViewOf(
  runs: Readonly<Record<string, OvernightRun>>,
  plans: Readonly<Record<string, Plan>>,
  requestId: string,
): PhaseView | null {
  const run = runOf(runs, requestId);
  if (!run) return null;
  const steps = runSteps(run, plans);
  const title = `Overnight · ${run.name}`;
  if (runOver(run)) {
    const { done, worked } = phasesDone(steps);
    const summary = `${done} of ${worked} ${worked === 1 ? "phase" : "phases"} done`;
    return {
      title,
      mark: done === worked ? "✓" : "◐",
      word: summary,
      settled: true,
      startedAtMs: run.startedAtMs,
      endedAtMs: run.finishedAtMs,
      outcome: `${summary}.`,
    };
  }
  const current = currentStep(steps);
  const word =
    run.state === "windingDown" || run.state === "reporting"
      ? "Writing your report"
      : current
        ? phaseTitle(current, steps.length)
        : steps.length === 0
          ? "Writing the plan"
          : "working";
  return { title, mark: "", word, settled: false, startedAtMs: run.startedAtMs, endedAtMs: null, outcome: null };
}

/** The composer's pill during a run: where the run is, "Phase 2 of 3 · Fix". */
export type RunPill = {
  runId: string;
  label: string;
  /** The run's phases, done and in all, for the donut and the list. */
  done: number;
  total: number;
  steps: readonly { title: string; done: boolean; active: boolean }[];
};

/** Where an active run is, for the pill: the phase at work, and the phases of its plan. */
export function runPill(runs: Readonly<Record<string, OvernightRun>>, plans: Readonly<Record<string, Plan>>): RunPill | null {
  const run = Object.values(runs)
    .filter(runActive)
    .toSorted((a, b) => b.createdAtMs - a.createdAtMs)[0];
  if (!run) return null;
  if (run.state === "windingDown" || run.state === "reporting")
    return { runId: run.id, label: "Writing your report", done: 0, total: 0, steps: [] };
  const all = runSteps(run, plans);
  const current = currentStep(all);
  if (!current) return null;
  const steps = all
    .filter((step) => step.mark !== "skipped")
    .map((step) => ({ title: step.name, done: step.mark === "done", active: step === current }));
  return {
    runId: run.id,
    label: phaseTitle(current, all.length),
    done: steps.filter((step) => step.done).length,
    total: steps.length,
    steps,
  };
}
