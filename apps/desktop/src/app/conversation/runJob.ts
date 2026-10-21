import type { BoardJob } from "@/app/conversation/boardJob";
import { deadlineLabel, type OvernightCardModel } from "@/app/conversation/overnightAdapter";
import { phaseOutcome, type RunStep, runOver, type StepMark } from "@/app/conversation/phaseView";
import type { JobOutcomeStatus, JobStage, JobStageState } from "@/components/conversation/elements/job-progress";
import { formatTime } from "@/lib/format";

/**
 * An overnight run as the session's job (the JobCard, in every state of the run): before the
 * start its phases listed with Start and Edit; at work the phase at work and the one after it,
 * with when it wraps up and Stop; once over, how it came out, the time it took and Merge.
 */
export type RunJob = {
  title: string;
  proposed: boolean;
  stages: JobStage[];
  stageIndex: number;
  stageProgress: number;
  /** "3 phases", before the start. */
  meta?: string;
  outcome?: { status: JobOutcomeStatus; summary?: string };
  elapsedMs?: number;
  detail?: string;
  warning?: string;
  /** Before the start: whether Start can go (nothing in the restrictions stops it). */
  start: { allowed: boolean } | null;
  /** At work and not ending yet: the head's Stop. */
  stop: boolean;
  /** A run done in full: Merge in the card. */
  merge: { sha: string; merged: boolean } | null;
};

/** A phase's mark as a step of the job; a phase the run ended in is where it stopped. */
function stageState(mark: StepMark, over: boolean): JobStageState {
  switch (mark) {
    case "done":
      return "done";
    case "blocked":
      return "failed";
    case "partial":
      return "partial";
    case "working":
      return over ? "pending" : "active";
    default:
      return "pending";
  }
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** "Not merged yet · 1 thing waits on you". */
function mergeWords(model: OvernightCardModel): string {
  const { run, details } = model;
  if (run.merged) return `Merged into ${run.merged.into}`;
  if (!details.verifiedSha) return "Nothing to merge yet";
  const waits = details.waiting === 0 ? "" : ` · ${details.waiting} ${details.waiting === 1 ? "thing waits" : "things wait"} on you`;
  return `Not merged yet${waits}`;
}

/** The one thing to know before the start, the most important first: what stops Start, then power. */
function warningOf(model: OvernightCardModel): string | undefined {
  const { run, details } = model;
  const power = details.power;
  if (run.problems[0]) return run.problems[0].message;
  if (power?.lidWillPause) return power.lowBattery ? "The battery is low: closing the lid will pause the run" : "Closing the lid will pause the run";
  if (power?.onBattery) return "On battery: plug in to be safe";
  return undefined;
}

/** How a run that is over came out: in full, with a phase failed, stopped, or partly. */
function outcomeOf(model: OvernightCardModel, worked: readonly RunStep[]): JobOutcomeStatus {
  const { run } = model;
  if (worked.length > 0 && worked.every((step) => step.mark === "done")) return "success";
  if (worked.some((step) => step.mark === "blocked") || run.stop?.type === "failed" || run.stop?.type === "blocked") return "failed";
  if (run.stop?.type === "stopped" || run.stop?.type === "stopDirective") return "cancelled";
  return "partial";
}

export function runJob(model: OvernightCardModel): RunJob {
  const { run, details } = model;
  const proposed = run.state === "proposed";
  const over = runOver(run);
  // Phases the run's restrictions leave out are no step of it.
  const worked = details.steps.filter((step) => step.mark !== "skipped");
  // A bare goal: the run's thread writes the plan first.
  const bare = details.steps.length === 0;
  const working = worked.findIndex((step) => step.mark === "working");
  const unfinished = worked.findIndex((step) => step.mark !== "done");
  const at = working !== -1 ? working : unfinished !== -1 ? unfinished : worked.length;
  const step = worked[at];
  const share = step ? details.phaseProgress[step.number] : undefined;
  const stages: JobStage[] = bare
    ? [{ name: "Write the plan", weight: 1, state: proposed || over ? "pending" : "active" }]
    : worked.map((item) => ({ name: item.name, weight: 1, state: proposed ? "pending" : stageState(item.mark, over) }));
  const base = {
    proposed,
    stages,
    stageIndex: proposed ? 0 : bare ? 0 : at,
    stageProgress: share?.workerTaskIds?.length ? (share.workersDone ?? 0) / share.workerTaskIds.length : 0,
    start: null,
    stop: false,
    merge: null,
  };

  if (proposed) {
    const deadline = run.directives.deadline;
    return {
      ...base,
      title: run.name,
      ...(bare ? {} : { meta: plural(worked.length, "phase") }),
      detail: deadline.type === "at" ? `Report ready at ${deadline.time.localTime.replace(/^0(?=\d:)/, "")}` : `Runs ${deadlineLabel(model)}`,
      ...(warningOf(model) ? { warning: warningOf(model)! } : {}),
      start: { allowed: run.problems.length === 0 },
    };
  }

  if (!over) {
    const ending = run.state === "windingDown" || run.state === "reporting";
    const detail = ending
      ? "Writing your report"
      : run.state === "preparing"
        ? "Preparing the worktree"
        : bare
          ? "Writing the plan"
          : run.windDownAtMs !== null
            ? `Wraps up at ${formatTime(run.windDownAtMs)}`
            : `Runs ${deadlineLabel(model)}`;
    return {
      ...base,
      title: bare ? run.name : `${run.name} · ${Math.min(at + 1, worked.length)} of ${worked.length}`,
      detail,
      stop: !ending,
    };
  }

  const status = outcomeOf(model, worked);
  const elapsed = run.startedAtMs !== null && run.finishedAtMs !== null ? { elapsedMs: run.finishedAtMs - run.startedAtMs } : {};
  if (status === "success") {
    const sha = details.verifiedSha;
    return {
      ...base,
      ...elapsed,
      title: run.name,
      outcome: {
        status,
        summary: details.outcome?.[0] ?? (worked.length === 1 ? "The phase is done." : `All ${worked.length} phases are done.`),
      },
      detail: mergeWords(model),
      merge: sha ? { sha, merged: run.merged?.verifiedCommit === sha } : null,
    };
  }
  const blocked = worked.find((item) => item.mark === "blocked");
  const partial = worked.find((item) => item.mark === "partial");
  const phase = (blocked ?? partial ?? step)?.number;
  const where = phase === undefined ? "" : ` at phase ${phase}`;
  const lastDone = worked.findLast((item) => item.mark === "done")?.number;
  const said = (item: RunStep | undefined) => (item ? phaseOutcome(run, item, true) || undefined : undefined);
  const [title, detail] =
    status === "failed"
      ? [
          blocked ? `Phase ${blocked.number} failed` : "The run failed",
          said(blocked) ?? (run.stop?.type === "failed" ? run.stop.message : details.outcome?.[0]),
        ]
      : status === "cancelled"
        ? run.stop?.type === "stopDirective"
          ? [lastDone === undefined ? "Stopped as you asked" : `Stopped after phase ${lastDone}`, "It stopped where you asked. The work so far is saved."]
          : [`Stopped${where}`, "You stopped it. The work so far is saved."]
        : partial
          ? [`Phase ${partial.number} partly done`, said(partial)]
          : [`Time ran out${where}`, "Its time was up. The work so far is saved."];
  return {
    ...base,
    ...elapsed,
    title,
    outcome: { status },
    ...(detail ? { detail } : {}),
  };
}

/**
 * The run that holds the session's job card: the newest, while it is proposed or at work, and
 * once over until its work is merged or the session starts new work after it ended.
 */
export function runInSlot(models: readonly OvernightCardModel[], job: BoardJob | null): OvernightCardModel | null {
  const model = models.findLast((candidate) => candidate.run.state !== "superseded");
  if (!model) return null;
  const { run } = model;
  if (!runOver(run)) return model;
  if (run.merged) return null;
  if (job && run.finishedAtMs !== null && job.atMs > run.finishedAtMs) return null;
  return model;
}
