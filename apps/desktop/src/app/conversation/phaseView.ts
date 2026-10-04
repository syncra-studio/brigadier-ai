import type { OvernightPhase, OvernightRun, PhaseState } from "@/ipc/generated";

/**
 * An overnight phase as everything that names it says it: the run card, the phase's block in
 * the thread and the composer's pill use the same title, mark and words (and so does the morning
 * report, from the daemon).
 */

/** The mark beside a phase's state: what it came to, never colour alone. */
export const PHASE_MARKS: Record<PhaseState, string> = {
  pending: "–",
  running: "",
  checking: "",
  verified: "✓",
  partial: "◐",
  blocked: "✕",
  skipped: "–",
};

/** Whether the run is over: nothing more happens in any of its phases. */
export function runOver(run: OvernightRun): boolean {
  return run.state === "finished" || run.state === "superseded";
}

/** A phase's state in a word or two; `over` once the run has ended. */
export function phaseWord(state: PhaseState, over: boolean): string {
  switch (state) {
    case "pending":
      return over ? "not reached" : "not started";
    case "running":
      return over ? "stopped" : "working";
    case "checking":
      return over ? "stopped" : "checking";
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

function plural(count: number, one: string, many: string): string {
  return `${count} ${count === 1 ? one : many}`;
}

function outcomeOf(run: OvernightRun, phase: OvernightPhase, over: boolean): string | null {
  const met = phase.criteria.filter((criterion) => criterion.status === "met").length;
  const total = phase.doneWhen.length || phase.criteria.length;
  switch (phase.state) {
    case "verified":
      return `${met} of ${plural(total, "done-when criterion", "done-when criteria")} met${
        phase.verifiedCommit ? ` · verified at ${phase.verifiedCommit.slice(0, 7)}` : ""
      }.`;
    case "partial":
    case "blocked":
      return phase.gaps[0] ?? `${met} of ${plural(total, "done-when criterion", "done-when criteria")} met.`;
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
        mark: PHASE_MARKS[phase.state],
        word: phaseWord(phase.state, over),
        settled,
        startedAtMs: phase.startedAtMs,
        endedAtMs: phase.settledAtMs ?? (settled ? run.finishedAtMs : null),
        outcome: settled ? outcomeOf(run, phase, over) : null,
      };
    }
    const planning = run.planning;
    if (planning?.requestId === requestId) {
      const settled = phaseSettled(planning.state, over);
      return {
        title: "Phase 0 · Write the plan",
        mark: PHASE_MARKS[planning.state],
        word: phaseWord(planning.state, over),
        settled,
        startedAtMs: planning.startedAtMs,
        endedAtMs: planning.settledAtMs ?? (settled ? run.finishedAtMs : null),
        outcome: !settled
          ? null
          : planning.state === "verified"
            ? `Wrote and checked a plan of ${plural(run.phases.length, "phase", "phases")}.`
            : (planning.gaps[0] ?? "The plan wasn't settled."),
      };
    }
  }
  return null;
}
