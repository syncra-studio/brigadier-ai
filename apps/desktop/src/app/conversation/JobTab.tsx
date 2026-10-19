import type { ReactNode } from "react";

import { type Job, type JobStep, phaseWords } from "@/app/conversation/job";
import { Timeline, type TimelineEvent } from "@/components/assistant-ui/elements/timeline";
import { formatTime } from "@/lib/format";

/** A step's line under its name: what happened, or that it comes next. */
function stepDetail(step: JobStep, next: boolean): string | undefined {
  if (step.summary) return step.summary;
  if (step.state === "active") return step.count;
  return next ? "Next" : undefined;
}

/**
 * The Job card's tab: every step of the job on a timeline (Q88), each with when it ran, what
 * happened and what comes next; under a step, what it holds in full (`details`: a review's
 * findings, the checks of a merge).
 */
export function JobTab({ job, details }: { job: Job; details?: Readonly<Record<string, ReactNode>> }) {
  const next = job.steps.find((step) => step.state === "pending");
  const events: TimelineEvent[] = job.steps.map((step) => {
    const at = step.startedAtMs ?? step.endedAtMs;
    return {
      id: step.id,
      when: step.state === "pending" ? "future" : step.state === "active" ? "now" : "past",
      time: at ? formatTime(at) : "",
      title: step.state === "failed" ? (step.failedLabel ?? `${step.name} failed`) : step.name,
      detail: stepDetail(step, step === next),
      more: details?.[step.id],
      failed: step.state === "failed",
    };
  });
  const phase = phaseWords(job);
  return (
    <div data-slot="job-tab" className="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <div className="max-w-thread mx-auto flex w-full flex-col gap-4 px-5 pt-6 pb-10">
        {phase && (
          <h2 className="text-muted-foreground text-label">
            {phase} · <span className="text-foreground">{job.phase?.title}</span>
          </h2>
        )}
        <Timeline events={events} />
      </div>
    </div>
  );
}
