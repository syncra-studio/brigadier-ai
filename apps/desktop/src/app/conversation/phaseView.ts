import type { OvernightPhase, OvernightRun, PhaseState, Plan, Task } from "@/ipc/generated";

/**
 * An overnight phase as everything that names it says it: the run card, the phase's block in
 * the thread and the composer's pill use the same title, mark and words (and so does the morning
 * report, from the daemon).
 */

/** The mark beside a phase's state: what it came to, never colour alone. */
export const PHASE_MARKS: Record<PhaseState, string> = {
  pending: "",
  running: "",
  checking: "",
  verified: "✓",
  partial: "◐",
  blocked: "✕",
  skipped: "–",
};

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

/**
 * Reports rendered again in the current shape, by message id: a finished run whose report was
 * written in an older one carries it as `reportText` (its message stays as written).
 */
export function reportTexts(runs: Readonly<Record<string, OvernightRun>> | undefined): Readonly<Record<string, string>> {
  const texts: Record<string, string> = {};
  for (const run of Object.values(runs ?? {})) {
    if (run.reportMessageId && run.reportText) texts[run.reportMessageId] = run.reportText;
  }
  return texts;
}

/**
 * The text each message shows (and Copy copies): a report rendered again over its message's
 * text, else the full text of a long message. The same object when nothing changes.
 */
export function shownTexts(
  fullText: Readonly<Record<string, string>>,
  reports: Readonly<Record<string, string>>,
): Readonly<Record<string, string>> {
  return Object.keys(reports).length === 0 ? fullText : { ...fullText, ...reports };
}

/** Whether the run is over: nothing more happens in any of its phases. */
export function runOver(run: OvernightRun): boolean {
  return run.state === "finished" || run.state === "superseded";
}

/** The mark beside a phase's state; a phase the run's end cut off reads "◐ unfinished". */
export function phaseMark(state: PhaseState, over: boolean): string {
  return over && (state === "running" || state === "checking") ? "◐" : PHASE_MARKS[state];
}

/** A phase's state in a word or two; `over` once the run has ended. */
export function phaseWord(state: PhaseState, over: boolean): string {
  switch (state) {
    case "pending":
      return over ? "not reached" : "not started";
    case "running":
      return over ? "unfinished" : "working";
    // Only older runs checked a whole phase: it was still at work.
    case "checking":
      return over ? "unfinished" : "working";
    case "verified":
      return "verified";
    case "partial":
      return "partial";
    case "blocked":
      return "blocked";
    case "skipped":
      return "skipped";
  }
}

/** Whether a phase has come to its end: it was settled, or the run ended around it. */
export function phaseSettled(state: PhaseState, over: boolean): boolean {
  return over || (state !== "pending" && state !== "running" && state !== "checking");
}

/** A phase's block in the thread: its header and, once settled, the one line it folds to. */
export type PhaseView = {
  title: string;
  mark: string;
  word: string;
  settled: boolean;
  startedAtMs: number | null;
  /** When it settled; the run's end when the run ended first. */
  endedAtMs: number | null;
  /** What it came to, in a sentence, once settled. */
  outcome: string | null;
};

/**
 * The run's stored words about the user, as said to the user (the daemon's `TO_YOU`, in
 * `wind_down.rs`): a phase's gaps keep them for its later leads.
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

/** How the gap of a phase the run's end cut off begins (the daemon's `CUT_OFF`, in `wind_down.rs`). */
const CUT_OFF = "It wasn't finished: ";
/** The same, as older runs recorded it. */
const OLD_CUT_OFF = "Its whole-phase checks never passed: ";

/**
 * Why a settled phase isn't verified: wind-down's gap when the run's end cut it off (it comes
 * last, after gaps kept from an earlier segment), else the first left of it.
 */
function phaseGap(gaps: readonly string[]): string | undefined {
  const last = gaps.at(-1);
  if (last?.startsWith(OLD_CUT_OFF)) return CUT_OFF + last.slice(OLD_CUT_OFF.length);
  return last?.startsWith(CUT_OFF) ? last : gaps[0];
}

function plural(count: number, one: string, many: string): string {
  return `${count} ${count === 1 ? one : many}`;
}

/** What came of a phase in a line, as its block and the run card say it; null while it works. */
export function phaseOutcome(run: OvernightRun, phase: OvernightPhase, over: boolean): string | null {
  const met = phase.criteria.filter((criterion) => criterion.status === "met").length;
  const total = phase.doneWhen.length || phase.criteria.length;
  switch (phase.state) {
    // An older run counted the criteria its checks met; a phase now is verified as a whole.
    case "verified":
      return phase.criteria.length > 0
        ? `${met} of ${plural(total, "done-when criterion", "done-when criteria")} met${
            phase.verifiedCommit ? ` · verified at ${phase.verifiedCommit.slice(0, 7)}` : ""
          }.`
        : phase.verifiedCommit
          ? `Verified at ${phase.verifiedCommit.slice(0, 7)}.`
          : "Verified.";
    case "partial":
    case "blocked":
      return toYou(
        phaseGap(phase.gaps) ??
          (phase.criteria.length > 0
            ? `${met} of ${plural(total, "done-when criterion", "done-when criteria")} met.`
            : "Part of it is left."),
      );
    case "skipped":
      return "Skipped, as the run's restrictions said.";
    case "pending":
      return over ? "Not reached: the run ended first." : null;
    case "running":
    case "checking":
      return over ? `The run ended during this phase (${run.stop?.type === "stopped" ? "stopped by you" : "its time was up"}).` : null;
  }
}

/** The phase whose block shows `requestId`'s work, as the thread shows it; Phase 0 included. */
export function phaseViewOf(runs: Readonly<Record<string, OvernightRun>>, requestId: string): PhaseView | null {
  for (const run of Object.values(runs)) {
    const over = runOver(run);
    const phase = run.phases.find((candidate) => candidate.requestId === requestId);
    if (phase) {
      const settled = phaseSettled(phase.state, over);
      return {
        title: `Phase ${phase.number} · ${phase.name}`,
        mark: phaseMark(phase.state, over),
        word: phaseWord(phase.state, over),
        settled,
        startedAtMs: phase.startedAtMs,
        endedAtMs: phase.settledAtMs ?? (settled ? run.finishedAtMs : null),
        outcome: settled ? phaseOutcome(run, phase, over) : null,
      };
    }
    const planning = run.planning;
    if (planning?.requestId === requestId) {
      const settled = phaseSettled(planning.state, over);
      return {
        title: "Phase 0 · Write the plan",
        mark: phaseMark(planning.state, over),
        word: phaseWord(planning.state, over),
        settled,
        startedAtMs: planning.startedAtMs,
        endedAtMs: planning.settledAtMs ?? (settled ? run.finishedAtMs : null),
        outcome: !settled
          ? null
          : planning.state === "verified"
            ? `Wrote and checked a plan of ${plural(run.phases.length, "phase", "phases")}.`
            : toYou(planning.gaps[0] ?? "The plan wasn't settled."),
      };
    }
  }
  return null;
}

/** A step's task that ended without its work done. */
const ENDED: ReadonlySet<Task["state"]> = new Set(["stopped", "failed", "rejected"]);

/** A run at work: started and not over yet. */
export function runActive(run: OvernightRun): boolean {
  return run.state !== "proposed" && !runOver(run);
}

/** The composer's pill during a run: where the run is, "Phase 2 of 3 · Fix · 2 of 4 steps". */
export type RunPill = {
  runId: string;
  label: string;
  /** The active phase's plan steps, done and in all, for the donut and the list. */
  done: number;
  total: number;
  steps: readonly { title: string; done: boolean; active: boolean }[];
};

/**
 * Where an active run is, for the pill: its phase at work (Phase 0 while it writes the plan),
 * and the steps of that phase's approved plan that are done. A lead's plan inside the phase is
 * that phase's steps, never the run's.
 */
export function runPill(
  runs: Readonly<Record<string, OvernightRun>>,
  plans: Readonly<Record<string, Plan>>,
  tasks: Readonly<Record<string, Task>>,
): RunPill | null {
  const run = Object.values(runs)
    .filter(runActive)
    .toSorted((a, b) => b.createdAtMs - a.createdAtMs)[0];
  if (!run) return null;
  if (run.state === "windingDown" || run.state === "reporting")
    return { runId: run.id, label: "Writing your report", done: 0, total: 0, steps: [] };
  const index = run.phases.findIndex((phase) => phase.state === "running" || phase.state === "checking");
  const phase = run.phases[index];
  const requestId = phase?.requestId ?? run.planning?.requestId ?? null;
  const where = phase
    ? `Phase ${phase.number === index + 1 ? `${phase.number} of ${run.phases.length}` : phase.number} · ${phase.name}`
    : run.planning && run.state === "planning"
      ? "Phase 0 · Write the plan"
      : null;
  if (!where) return null;
  const plan = Object.values(plans)
    .filter((candidate) => requestId !== null && candidate.requestId === requestId && candidate.state.type === "approved")
    .toSorted((a, b) => b.createdAtMs - a.createdAtMs)[0];
  const steps = (plan?.steps ?? []).map((step) => {
    const task = step.taskId ? tasks[step.taskId] : undefined;
    const done = task?.state === "landed" || task?.state === "done";
    const active = task !== undefined && !done && !ENDED.has(task.state);
    return { title: step.title, done, active };
  });
  const done = steps.filter((step) => step.done).length;
  const label = steps.length > 0 ? `${where} · ${done} of ${steps.length} steps` : where;
  return { runId: run.id, label, done, total: steps.length, steps };
}
