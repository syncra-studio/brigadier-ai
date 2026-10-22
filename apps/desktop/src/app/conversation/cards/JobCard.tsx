import { type ReactNode, useState } from "react";

import type { PlanBar } from "@/app/conversation/boardJob";
import type { LiveAction, LiveJob } from "@/app/conversation/liveJob";
import type { OvernightActions, OvernightCardModel } from "@/app/conversation/overnightAdapter";
import { runJob } from "@/app/conversation/runJob";
import { useAction } from "@/app/conversation/useAction";
import { useRunCommand } from "@/app/conversation/useRunCommand";
import { JobProgress, ScheduleCard } from "@/components/conversation";
import { Button } from "@/components/controls";
import { ConfirmDialog } from "@/components/overlays";
import { openUrl } from "@/ipc/client";
import { cn } from "@/lib/utils";
import { cancelMerge, mergeAnyway } from "@/state/actions";

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

/** How long the live job has gone on, or took: none while it waits on the user. */
function elapsedOf(live: LiveJob, nowMs: number): number | undefined {
  if (live.sinceMs === undefined || live.tone === "waiting") return undefined;
  if (live.endedAtMs !== undefined) return live.endedAtMs > live.sinceMs ? live.endedAtMs - live.sinceMs : undefined;
  return live.tone === "running" ? nowMs - live.sinceMs : undefined;
}

/**
 * The live job's buttons (R2): Open CI for the push CI checks, Cancel for the merge held until
 * workers finish (they were the composer rail's pills; now they are the card's), and Merge
 * anyway for a merge the merge rule refused.
 */
export function LiveButtons({ conversationId, actions }: { conversationId: string; actions: readonly LiveAction[] }) {
  const merge = useAction();
  if (actions.length === 0) return null;
  return (
    <>
      {actions.map((action) => {
        switch (action.type) {
          case "openCi":
            return (
              <Button key={action.type} type="button" size="xs" variant="secondary" data-slot="job-open-ci" onClick={() => void openUrl(action.url).catch(() => {})}>
                Open CI
              </Button>
            );
          case "cancelMerge":
            return (
              <Button
                key={action.type}
                type="button"
                size="xs"
                variant="secondary"
                data-slot="merge-cancel"
                disabled={merge.busy}
                onClick={() => merge.run(() => cancelMerge(conversationId))}
              >
                Cancel
              </Button>
            );
          case "mergeAnyway":
            return (
              <Button
                key={action.type}
                type="button"
                size="xs"
                variant="secondary"
                data-slot="merge-anyway"
                disabled={merge.busy}
                onClick={() => merge.run(() => mergeAnyway(conversationId))}
              >
                Merge anyway
              </Button>
            );
        }
      })}
      {merge.error && (
        <p role="alert" className="text-destructive text-xs">
          {merge.error}
        </p>
      )}
    </>
  );
}

/**
 * The session's Job card (J1): what is happening right now, in one headline and one detail line
 * (J6), with the buttons its state needs. Only a real plan of several steps adds its phase bar
 * (mockup 06): one segment per step, the step at work before the next. A click opens every step
 * in a tab.
 */
export function JobCard({
  conversationId,
  live,
  plan,
  nowMs,
  onOpen,
}: {
  conversationId: string;
  live: LiveJob | null;
  plan?: PlanBar | null | undefined;
  nowMs: number;
  onOpen?: () => void;
}) {
  const said = live && !(plan && live.plain) ? live : null;
  const title = said?.headline ?? plan?.title ?? "";
  const detail = plan ? [plan.words, said?.detail].filter(Boolean).join(" · ") : said?.detail;
  const failedAt = plan ? plan.stages.findIndex((stage) => stage.state === "failed") : -1;
  return (
    <JobSurface label={`${title}: see every step`} onOpen={onOpen}>
      <JobProgress
        data-live={live?.tone}
        title={title}
        tone={live?.tone}
        status={live ? title : undefined}
        stages={plan?.stages ?? []}
        stageIndex={plan?.index ?? 0}
        stageProgress={0}
        stalled={!live && plan ? !plan.stages.some((stage) => stage.state === "active") && failedAt !== -1 : false}
        elapsedMs={live ? elapsedOf(live, nowMs) : undefined}
        detail={detail || undefined}
        actions={live && live.actions.length > 0 ? <LiveButtons conversationId={conversationId} actions={live.actions} /> : undefined}
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
  conversationId = "",
  live,
  nowMs = 0,
  onOpen,
  onEdit,
}: {
  model: OvernightCardModel;
  /** The session's id, for the live buttons. */
  conversationId?: string | undefined;
  actions: OvernightActions;
  /** What the session waits on or does right now: its words go over the run's own (J5), its buttons with them. */
  live?: LiveJob | null | undefined;
  nowMs?: number | undefined;
  onOpen?: () => void;
  onEdit: () => void;
}) {
  const job = runJob(model);
  const said = live && !live.plain ? live : null;
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
  const liveButtons = said && said.actions.length > 0 ? <LiveButtons conversationId={conversationId} actions={said.actions} /> : null;
  return (
    <>
      <JobSurface label={`${job.title}: see the run`} onOpen={onOpen}>
        <JobProgress
          data-run={run.id}
          title={said?.headline ?? job.title}
          tone={said?.tone}
          status={said?.headline}
          stages={job.stages}
          stageIndex={job.stageIndex}
          stageProgress={job.stageProgress}
          proposed={job.proposed}
          meta={job.meta}
          outcome={job.outcome}
          elapsedMs={said ? elapsedOf(said, nowMs) : job.elapsedMs}
          detail={said?.detail ?? job.detail}
          warning={job.warning}
          onCancel={job.stop && !stopped ? () => setAsking(true) : undefined}
          actions={
            buttons || liveButtons ? (
              <>
                {buttons}
                {liveButtons}
              </>
            ) : null
          }
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
