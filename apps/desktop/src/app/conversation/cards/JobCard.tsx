import { type ReactNode, useState } from "react";

import { type Job, jobLabel } from "@/app/conversation/job";
import type { OvernightActions, OvernightCardModel } from "@/app/conversation/overnightAdapter";
import { runJob } from "@/app/conversation/runJob";
import { useRunCommand } from "@/app/conversation/useRunCommand";
import { JobProgress, ScheduleCard } from "@/components/conversation";
import { Button } from "@/components/controls";
import { ConfirmDialog } from "@/components/overlays";
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
 * The job card's surface: a click anywhere on it opens every step in a tab. The opening button
 * lies under the card's words, which let clicks through to it; the card's own buttons (Stop,
 * Start, Merge) sit beside it, so pressing one never opens the tab.
 */
function JobSurface({ label, onOpen, children }: { label: string; onOpen?: (() => void) | undefined; children: ReactNode }) {
  return (
    <StackCard className="relative">
      <button
        type="button"
        data-slot="job-card-open"
        aria-label={label}
        title="See every step"
        onClick={onOpen}
        className="rounded-summary hover:bg-foreground/8 active:bg-foreground/10 focus-visible:ring-ring absolute inset-0 transition-colors outline-none focus-visible:ring-1"
      />
      <div data-slot="job-card" className="text-label pointer-events-none relative flex min-w-0 flex-col gap-2 px-5 py-3.5 text-start">
        {children}
      </div>
    </StackCard>
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
  const title = jobLabel(job);
  return (
    <JobSurface label={`${title}: see every step`} onOpen={onOpen}>
      <JobProgress
        title={title}
        stageIndex={index}
        stageProgress={current?.state === "active" ? share(current.count) : 0}
        stages={job.steps.map((step, i) => ({
          name: step.name,
          weight: 1,
          state: step.state,
          description: (i === index && step.state === "active") || step.state === "failed" ? step.summary : undefined,
        }))}
      />
    </JobSurface>
  );
}

/**
 * An overnight run as the session's job (#385, every state of the run): its phases listed with
 * Start and Edit before the start, the phase at work with when it wraps up and Stop (which asks
 * first) while it works, how it came out with Merge once it is over. A click opens the run's part
 * of the job's tab: each phase's scope and "done when", then its report.
 */
export function RunJobCard({
  model,
  actions,
  onOpen,
  onEdit,
}: {
  model: OvernightCardModel;
  actions: OvernightActions;
  onOpen?: () => void;
  onEdit: () => void;
}) {
  const job = runJob(model);
  const { run } = model;
  const command = useRunCommand(run);
  const [asking, setAsking] = useState(false);
  const [stopped, setStopped] = useState(false);
  const buttons = job.start ? (
    <>
      <Button type="button" size="xs" disabled={command.busy || !job.start.allowed} onClick={() => command.send("start", actions.start)}>
        Start
      </Button>
      <Button type="button" size="xs" variant="secondary" onClick={onEdit}>
        Edit
      </Button>
    </>
  ) : job.merge ? (
    <Button
      type="button"
      size="xs"
      disabled={command.busy || job.merge.merged}
      onClick={() => job.merge && command.send("merge", (sent) => actions.merge(sent, job.merge!.sha))}
    >
      {job.merge.merged ? "Merged" : "Merge"}
    </Button>
  ) : null;
  return (
    <>
      <JobSurface label={`${job.title}: see the run`} onOpen={onOpen}>
        <JobProgress
          data-run={run.id}
          title={job.title}
          stages={job.stages}
          stageIndex={job.stageIndex}
          stageProgress={job.stageProgress}
          proposed={job.proposed}
          meta={job.meta}
          outcome={job.outcome}
          elapsedMs={job.elapsedMs}
          detail={job.detail}
          warning={job.warning}
          onCancel={job.stop && !stopped ? () => setAsking(true) : undefined}
          actions={buttons}
        />
        {command.error && (
          <p role="alert" className="text-destructive text-xs">
            {command.error}
          </p>
        )}
      </JobSurface>
      <ConfirmDialog
        open={asking}
        onOpenChange={setAsking}
        title="Stop the overnight run?"
        description="Its workers stop now. The work so far is saved on the run's branch, and you get the report."
        confirm="Stop"
        destructive
        busy={command.busy}
        onConfirm={() =>
          command.send("stop", actions.stop, () => {
            setStopped(true);
            setAsking(false);
          })
        }
      >
        {command.error && (
          <p role="alert" className="text-destructive text-xs">
            {command.error}
          </p>
        )}
      </ConfirmDialog>
    </>
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
