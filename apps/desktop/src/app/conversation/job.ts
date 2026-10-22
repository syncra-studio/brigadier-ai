import type { JobStageState } from "@/components/conversation/elements/job-progress";

/**
 * A session's job (Q88): the steps it works through, in order. For a change: Building, CI,
 * Merging; for a phased plan, the steps of the phase at work. Its tab
 * shows it step by step; the Job card says what is happening now instead (liveJob.ts).
 */

export type JobStepState = JobStageState;

export interface JobStep {
  id: string;
  /** "Building", "CI", a phase's step. */
  name: string;
  state: JobStepState;
  /** How far the step at work is: "3 of 5". */
  count?: string | undefined;
  /** Said when it failed: "Checks failed" (`${name} failed` by default). */
  failedLabel?: string | undefined;
  startedAtMs?: number | null | undefined;
  endedAtMs?: number | null | undefined;
  /** What happened, in a line: "2 workers built it", "CI passed". */
  summary?: string | undefined;
}

export interface Job {
  /** The phase at work of a phased plan. */
  phase?: { number: number; of: number; title: string } | undefined;
  steps: readonly JobStep[];
}

/** "Phase 2 of 4". */
export function phaseWords(job: Job): string | null {
  return job.phase ? `Phase ${job.phase.number} of ${job.phase.of}` : null;
}

/**
 * The card's one line: the step at work and how far it is ("Building · 3 of 5"); a failure
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
