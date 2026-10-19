import type { ReactNode } from "react";

import { type Job, jobLabel } from "@/app/conversation/job";
import { JobProgress } from "@/components/assistant-ui/elements/job-progress";
import { ScheduleCard } from "@/components/assistant-ui/elements/schedule-card";
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

/**
 * The session's job while it works through steps: one bar cut into its steps and one short
 * line ("Reviewing · 3 of 5", "Checks failed · fixing"). A click opens every step in a tab.
 */
export function JobCard({ job, onOpen }: { job: Job; onOpen?: () => void }) {
  const done = job.steps.filter((step) => step.state === "done").length;
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
          done={done}
          total={job.steps.length}
          stages={job.steps.map((step) => step.state)}
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
