import { Moon } from "@openai/apps-sdk-ui/components/Icon";
import { useRef, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { WorkerChip } from "@/app/conversation/WorkerChip";
import {
  deadlineLabel,
  overnightCommand,
  restrictionLines,
  type OvernightActions,
  type OvernightCardModel,
  type OvernightCommand,
} from "@/app/conversation/overnightAdapter";
import {
  AgentPlan,
  type AgentPlanStep,
  type AgentPlanStepStatus,
} from "@/components/assistant-ui/elements/agent-plan";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { PhaseState } from "@/ipc/generated";

const MARK: Record<PhaseState, AgentPlanStepStatus> = {
  pending: "pending",
  running: "active",
  checking: "active",
  verified: "done",
  partial: "partial",
  blocked: "failed",
  skipped: "skipped",
};

/** The same AgentPlan surface as a normal plan, with run facts and user-only commands. */
export function OvernightPlanCard({
  model,
  actions,
}: {
  model: OvernightCardModel;
  actions: OvernightActions | null;
}) {
  const { run, details } = model;
  const action = useAction();
  const locked = useRef(false);
  const retryCommands = useRef(new Map<string, OvernightCommand>());
  const [stopped, setStopped] = useState(false);
  const proposed = run.state === "proposed";
  const finished = run.state === "finished";
  const ending = run.state === "windingDown" || run.state === "reporting";
  const running = !proposed && !finished && run.state !== "superseded";
  const restrictions = restrictionLines(run);
  const power = proposed ? details.power : undefined;
  const send = (kind: string, job: (command: OvernightCommand) => Promise<unknown>, stop = false) => {
    if (locked.current) return;
    locked.current = true;
    const key = `${run.id}:${run.revision}:${run.generation}:${kind}`;
    const command = retryCommands.current.get(key) ?? overnightCommand(run);
    retryCommands.current.set(key, command);
    action.run(async () => {
      try {
        await job(command);
        retryCommands.current.delete(key);
        if (stop) setStopped(true);
      } finally {
        locked.current = false;
      }
    });
  };
  const steps: AgentPlanStep[] = run.phases.map((phase) => {
    const progress = details.phaseProgress[phase.id];
    const quota = progress?.quota;
    const workerTaskIds = progress?.workerTaskIds;
    const status = quota
      ? `Waiting for ${quota.provider} limits · resets ${new Date(quota.resetsAtMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false })}`
      : progress?.fixRound
        ? `Fixing ${progress.fixRound}/2`
        : phase.state === "running"
          ? "Lead working"
          : phase.state === "checking"
            ? "Checking"
            : phase.state;
    return {
      key: phase.id,
      title: `Phase ${phase.number} · ${phase.name}`,
      status: MARK[phase.state],
      statusLabel: proposed ? "" : status,
      live: running,
      folded: proposed || (phase.state !== "running" && phase.state !== "checking"),
      detail: (
        <div className="flex flex-col gap-1.5">
          {phase.scope && <p>{phase.scope}</p>}
          {phase.doneWhen.length > 0 && (
            <div>
              <p className="font-medium">Done when</p>
              <ul className="list-inside list-disc space-y-1">
                {phase.doneWhen.map((criterion) => (
                  <li key={criterion.id}>
                    {criterion.text}
                    {progress?.criteria?.[criterion.id] && ` · ${progress.criteria[criterion.id]}`}
                  </li>
                ))}
              </ul>
            </div>
          )}
          {Boolean(workerTaskIds?.length) && (
            <div className="flex flex-wrap gap-1">
              {workerTaskIds?.map((id) => (
                <WorkerChip key={id} taskId={id} />
              ))}
            </div>
          )}
        </div>
      ),
    };
  });
  if (steps.length === 0)
    steps.push({
      key: "phase-0",
      title: "Phase 0 · Write the plan",
      detail: run.goal,
      status: running ? "active" : "pending",
      statusLabel: running ? "Writing and reviewing the plan" : "Ready to plan",
      folded: !running,
    });

  return (
    <AgentPlan
      id={`overnight-${run.id}`}
      data-card="overnight-plan"
      tabIndex={-1}
      title={`${run.name} · ${deadlineLabel(model)}`}
      badges={
        <>
          {running && (
            <Moon aria-label="Running unattended" className="text-muted-foreground size-icon-sm" />
          )}
          {proposed && <Badge variant="warning">Proposed</Badge>}
          {finished && <Badge variant="secondary">Finished</Badge>}
        </>
      }
      steps={steps}
      showProgress={!proposed && run.phases.length > 0}
      progressLabel="verified"
      footer={
        <div className="flex flex-col gap-2">
          {!proposed && (
            <p className="text-muted-foreground text-xs tabular-nums" role="status">
              {details.waiting} Waiting · {details.decided} Decided
            </p>
          )}
          {ending && (
            <p role="status" className="text-muted-foreground text-xs">
              Saving progress and writing your report
            </p>
          )}
          {actions && (
            <div className="flex flex-wrap items-center gap-2">
              {proposed && (
                <Button
                  type="button"
                  size="sm"
                  disabled={action.busy || run.problems.length > 0}
                  onClick={() => send("start", actions.start)}
                >
                  Start
                </Button>
              )}
              {running && (
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={action.busy || ending || stopped}
                  onClick={() => send("stop", actions.stop, true)}
                >
                  {stopped ? "Stopping" : "Stop"}
                </Button>
              )}
              {finished && (
                <>
                  {details.reportMessageId && (
                    <Button
                      type="button"
                      size="sm"
                      variant="link"
                      onClick={() =>
                        actions.openReport(run.conversationId, details.reportMessageId!)
                      }
                    >
                      Read report
                    </Button>
                  )}
                  <Button
                    type="button"
                    size="sm"
                    disabled={action.busy || !details.verifiedSha || run.merged?.verifiedCommit === details.verifiedSha}
                    onClick={() =>
                      details.verifiedSha &&
                      send("merge", (command) => actions.merge(command, details.verifiedSha!))
                    }
                  >
                    {run.merged?.verifiedCommit === details.verifiedSha ? "Merged" : "Merge"}
                  </Button>
                  {details.remainingPhaseIds.length > 0 && (
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      disabled={action.busy}
                      onClick={() =>
                        send("continue", (command) => actions.continue(command, "continue until done"))
                      }
                    >
                      Continue
                    </Button>
                  )}
                </>
              )}
            </div>
          )}
          {finished && !details.verifiedSha && (
            <p className="text-muted-foreground text-xs">No verified work to merge yet.</p>
          )}
          {finished && run.notification?.deliveryError && !details.notificationsOff && (
            <p role="status" className="text-muted-foreground text-xs">
              The notification couldn’t be shown: {run.notification.deliveryError}
            </p>
          )}
          {action.error && (
            <p role="alert" className="text-destructive text-xs">
              {action.error}
            </p>
          )}
        </div>
      }
    >
      {finished && details.outcome && (
        <div className="flex flex-col gap-1 text-sm" role="status">
          {details.outcome.map((line, index) => (
            <p key={index}>{line}</p>
          ))}
        </div>
      )}
      {proposed && (
        <div className="flex flex-col gap-1 text-xs">
          {run.directives.deadline.type === "at" && (
            <p className="text-muted-foreground">
              Report ready {run.directives.deadline.time.day} ·{" "}
              {run.directives.deadline.time.localTime} ({run.directives.deadline.time.timeZone})
            </p>
          )}
          {(run.rules || run.sources.length > 0) && (
            <details className="text-muted-foreground">
              <summary className="cursor-pointer">Rules and sources</summary>
              {run.rules && <p className="whitespace-pre-wrap">{run.rules}</p>}
              {run.sources.map((source) => <p key={source.path}>{source.path}{source.sections ? ` · ${source.sections}` : ""}</p>)}
            </details>
          )}
          {restrictions.map((line) => (
            <p key={line}>{line}</p>
          ))}
          {/* Each line already reads "Ignored: … (Brigadier picks this itself)". */}
          {run.directives.ignored.map((line) => (
            <p key={line} className="text-muted-foreground">{line}</p>
          ))}
          {run.problems.map((problem, index) => (
            <p key={index} role="alert" className="text-warning">
              {problem.message}
            </p>
          ))}
          {power?.onBattery && !power.lowBattery && (
            <p className="text-warning">On battery: plug in to be safe</p>
          )}
          {power?.lidWillPause && (
            <p className="text-warning">
              {power.lowBattery
                ? "The battery is low: closing the lid will pause the run. Plug it in."
                : "Closing the lid will pause the run"}
            </p>
          )}
          {power?.lidWillPause && power.offerLidSetup && actions && (
            <Button
              type="button"
              variant="link"
              size="xs"
              className="self-start px-0"
              disabled={action.busy}
              onClick={() => send("lid", actions.setUpLidClosed)}
            >
              Keep running with the lid closed
            </Button>
          )}
        </div>
      )}
      {details.notificationsOff && (
        <p className="text-warning text-xs" role="status">
          {finished
            ? "Notifications are off for Brigadier, so the run’s notification wasn’t shown. "
            : "Notifications are off for Brigadier, so you won’t hear when the run finishes. "}
          {actions && (
            <Button
              type="button"
              variant="link"
              size="xs"
              className="h-auto px-0 align-baseline"
              disabled={action.busy}
              onClick={() => send("notifications", actions.openNotificationSettings)}
            >
              Turn them on
            </Button>
          )}
        </p>
      )}
      {running && (
        <p className="text-muted-foreground text-xs" role="status">
          {ending
            ? "Ending the run"
            : run.state === "waitingQuota"
              ? "Waiting for limits"
              : run.state === "preparing"
                ? "Preparing the worktree"
                : run.state === "planning"
                  ? "Writing the plan"
                  : run.state === "phaseGate"
                    ? "Checking the whole phase"
                    : "Working through the plan"}
        </p>
      )}
    </AgentPlan>
  );
}
