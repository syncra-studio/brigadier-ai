import type { ReactNode } from "react";

import { type Job, jobLabel } from "@/app/conversation/job";
import { JobProgress, ScheduleCard } from "@/components/conversation";
import { cn } from "@/lib/utils";

/**
 * A card of the summary's column beside the context card (Q88): the same surface, its own card.
 * It sits under the context card while that is open, and in its place while it is hidden.
 */
export function StackCard({ className, children }: { className?: string; children: ReactNode }) {
  return (
    <div className={cn("bg-popover rounded-summary shadow-summary pointer-events-auto flex min-w-0 flex-col", className)}>
      {children}
    </div>
  );
}

/** "3 of 5" as a share of the step done; nothing to count reads as a step just begun. */
function share(count: string | undefined): number {
  const [done, of] = (count?.match(/^(\d+) of (\d+)/) ?? []).slice(1).map(Number);
  return done !== undefined && of ? done / of : 0;
}

/**
 * The session's job while it works through steps: assistant-ui's job progress element with the
 * job's line ("Reviewing · 3 of 5", "Checks failed · fixing"), a bar of one segment per step, and
 * the step at work (or the one that failed) shimmering before the next. A click opens every step
 * in a tab.
 */
export function JobCard({ job, onOpen }: { job: Job; onOpen?: () => void }) {
  const at = job.steps.findIndex((step) => step.state === "active");
  const next = job.steps.findIndex((step) => step.state !== "done");
  const index = at !== -1 ? at : next !== -1 ? next : job.steps.length;
  const current = job.steps[index];
  return (
    <StackCard>
      <button
        type="button"
        data-slot="job-card"
        title="See every step"
        onClick={onOpen}
        className="rounded-summary hover:bg-foreground/8 active:bg-foreground/10 focus-visible:ring-ring flex min-w-0 px-5 py-3.5 text-start text-label transition-colors outline-none focus-visible:ring-1"
      >
        <JobProgress
          title={jobLabel(job)}
          stageIndex={index}
          stageProgress={current?.state === "active" ? share(current.count) : 0}
          stages={job.steps.map((step, i) => ({
            name: step.name,
            weight: 1,
            state: step.state,
            description: (i === index && step.state === "active") || step.state === "failed" ? step.summary : undefined,
          }))}
        />
      </button>
    </StackCard>
  );
}

/**
 * Something that waits on a set time, in the same column: a session paused until a usage limit
 * resets, an overnight run's planned end, a retry at a time. Only for a time the board holds.
 */
export function WaitCard({
  title,
  reason,
  atMs,
  nowMs,
}: {
  title: string;
  reason?: string | undefined;
  atMs: number;
  nowMs: number;
}) {
  return (
    <StackCard>
      <ScheduleCard className="px-5 py-3.5 text-label" title={title} reason={reason} atMs={atMs} nowMs={nowMs} />
    </StackCard>
  );
}
