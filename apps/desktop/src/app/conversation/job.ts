import type { JobStageState } from "@/components/assistant-ui/elements/job-progress";

/**
 * A session's job (Q88): the steps it works through, in order. For a change: Building,
 * Reviewing, Fixing, Checks, Merging; for a phased plan, the steps of the phase at work. The Job
 * card shows it while a step is left, in one bar and one short line; its tab, step by step.
 */

export type JobStepState = JobStageState;

export interface JobStep {
  id: string;
  /** "Reviewing", "Checks", a phase's step. */
  name: string;
  state: JobStepState;
  /** How far the step at work is: "3 of 5". */
  count?: string | undefined;
  /** Said when it failed: "Checks failed" (`${name} failed` by default). */
  failedLabel?: string | undefined;
  startedAtMs?: number | null | undefined;
  endedAtMs?: number | null | undefined;
  /** What happened, in a line: "Review found 3 issues, all fixed", "CI passed in 4m". */
  summary?: string | undefined;
}

export interface Job {
  /** The phase at work of a phased plan. */
  phase?: { number: number; of: number; title: string } | undefined;
  steps: readonly JobStep[];
}

/** A job is worth a card while one of its steps is not done. */
export function jobShown(job: Job | null | undefined): job is Job {
  return !!job && job.steps.some((step) => step.state !== "done");
}

/** "Phase 2 of 4". */
export function phaseWords(job: Job): string | null {
  return job.phase ? `Phase ${job.phase.number} of ${job.phase.of}` : null;
}

/**
 * The card's one line: the step at work and how far it is ("Reviewing · 3 of 5"); a failure
 * first, then what happens about it ("Checks failed · fixing"); a phased plan's phase before it.
 */
export function jobLabel(job: Job): string {
  const failed = job.steps.findLast((step) => step.state === "failed");
  const active = job.steps.find((step) => step.state === "active");
  const next = job.steps.find((step) => step.state === "pending");
  const step = failed
    ? [failed.failedLabel ?? `${failed.name} failed`, active?.name.toLowerCase()]
    : active
      ? [active.name, active.count]
      : [next?.name];
  return [phaseWords(job), ...step].filter(Boolean).join(" · ");
}
