import { Moon } from "@openai/apps-sdk-ui/components/Icon";
import { useContext, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import {
  phaseOutcome,
  phasesDone,
  phaseTitle,
  phaseWord,
  type RunStep,
  runOver,
  type StepMark,
} from "@/app/conversation/phaseView";
import { taskState } from "@/app/conversation/rowWords";
import { useAction } from "@/app/conversation/useAction";
import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
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
import { JobProgress, type JobOutcomeStatus } from "@/components/assistant-ui/elements/job-progress";
import { disclosureRow } from "@/components/assistant-ui/elements/surfaces";
import { Timeline, type TimelineEvent } from "@/components/assistant-ui/elements/timeline";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { OvernightPhase, OvernightRun } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";

const MARK: Record<StepMark, AgentPlanStepStatus> = {
  pending: "pending",
  working: "active",
  done: "done",
  partial: "partial",
  blocked: "failed",
  skipped: "skipped",
};

const NUL = "\u0000";

/** A phase's workers inside the run card, one line each, each opening its worker. */
function PhaseWork({ taskIds }: { taskIds: readonly string[] }) {
  const { setPanel } = useContext(AgentsPanelContext);
  // Each line as "task id NUL state NUL title", so the card re-renders only when one changes.
  const lines = useBoard(
    useShallow((s) =>
      taskIds.flatMap((id) => {
        const task = s.board?.tasks[id];
        // A check opens from what it checked; a phase's own checks list here.
        if (!task || (task.gateLink !== null && task.gateLink.owner.type !== "phase")) return [];
        return [[task.id, taskState(task).word, task.title].join(NUL)];
      }),
    ),
  );
  if (lines.length === 0) return null;
  return (
    <ul className="flex flex-col whitespace-normal">
      {lines.map((line) => {
        const [taskId = "", word = "", title = ""] = line.split(NUL);
        return (
          <li key={taskId} className="flex min-w-0">
            <button
              type="button"
              title={title}
              onClick={() => setPanel(taskId)}
              className="hover:text-foreground rounded-control flex min-w-0 items-center gap-1 text-start"
            >
              <span className="text-foreground/80 min-w-0 truncate">{title}</span>
              <span className="shrink-0">· {word}</span>
            </button>
          </li>
        );
      })}
    </ul>
  );
}

/** Which phases Merge takes (the done ones in a row from the first): "phase 1", "phases 1 and 2". */
function mergedPhases(steps: readonly RunStep[]): string | null {
  const numbers: number[] = [];
  for (const step of steps) {
    if (step.mark === "skipped") continue;
    if (step.mark !== "done") break;
    numbers.push(step.number);
  }
  if (numbers.length === 0) return null;
  if (numbers.length === 1) return `phase ${numbers[0]}`;
  return `phases ${numbers.slice(0, -1).join(", ")} and ${numbers.at(-1)}`;
}

/** The finished card's second line: what Merge takes, and where the work is. */
function whereLine(run: OvernightRun, steps: readonly RunStep[], verifiedSha: string | undefined): string {
  const branch = run.workspace ? `on ${run.workspace.branch}` : "";
  const phases = mergedPhases(steps);
  if (run.merged) return `Merged into ${run.workspace?.base ?? "the base branch"} · ${branch}`;
  if (!verifiedSha || !phases) return `Nothing to merge yet · ${branch}`;
  return `Merge takes ${phases} (${verifiedSha.slice(0, 7)}) · ${branch}`;
}

/** The same AgentPlan surface as a normal plan, with run facts and user-only commands. */
export function OvernightPlanCard({
  model,
  actions,
  className,
}: {
  model: OvernightCardModel;
  actions: OvernightActions | null;
  /** For the card's own surface, e.g. none inside the summary's card. */
  className?: string;
}) {
  const { run, details } = model;
  const action = useAction();
  const locked = useRef(false);
  const retryCommands = useRef(new Map<string, OvernightCommand>());
  const [stopped, setStopped] = useState(false);
  const proposed = run.state === "proposed";
  const finished = run.state === "finished";
  const over = runOver(run);
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
  // A started run reads as its progress over a timeline of its phases; a proposal lists them.
  const started = !proposed && details.steps.length > 0;
  const quotaWord = (step: RunStep) => {
    const quota = details.phaseProgress[step.number]?.quota;
    return quota && `Waiting for ${quota.provider ? `${quota.provider} ` : ""}limits · resets ${clockTime(quota.resetsAtMs)}`;
  };
  const steps: AgentPlanStep[] = started
    ? []
    : details.steps.map((step) => {
        const phase = sourcePhase(run, step);
        return {
          key: `phase-${step.number}`,
          title: `Phase ${step.number} · ${step.name}`,
          status: MARK[step.mark],
          statusLabel: proposed ? "" : quotaWord(step) || phaseWord(step.mark, over),
          live: running,
          folded: true,
          detail: (
            <div className="flex flex-col gap-1.5">
              {phase?.scope && <p>{phase.scope}</p>}
              <DoneWhen lines={phase?.doneWhen ?? []} />
            </div>
          ),
        };
      });
  // A bare goal: the run's thread writes the plan first.
  if (!started && steps.length === 0)
    steps.push({
      key: "plan",
      title: "Write the plan",
      detail: run.goal,
      status: running ? "active" : "pending",
      statusLabel: running ? "Writing the plan" : "Ready to plan",
      folded: !running,
    });

  return (
    <AgentPlan
      id={`overnight-${run.id}`}
      data-card="overnight-plan"
      className={className}
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
      showProgress={false}
      footer={
        <div className="flex flex-col gap-2">
          {!proposed && !finished && (
            <p className="text-muted-foreground text-xs tabular-nums" role="status">
              {details.waiting} waiting on you · {details.decided} decided for you
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
                  {details.remaining > 0 && (
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
      {started && (
        <>
          <RunProgress model={model} over={over} ending={ending} />
          <Timeline
            aria-label="Phases"
            events={details.steps.map((step) =>
              phaseEvent(run, step, over, quotaWord(step), details.phaseProgress[step.number]?.workerTaskIds ?? []),
            )}
          />
        </>
      )}
      {finished && (
        // What came of it, where the work is and what Merge takes, and (live) what waits on the user.
        <div className="flex flex-col gap-0.5" role="status">
          {details.outcome?.[0] && <p className="text-sm">{details.outcome[0]}</p>}
          <p className="text-muted-foreground truncate text-xs" title={run.workspace?.branch}>
            {/* The report's own second line, until the work is merged. */}
            {run.merged || !details.outcome?.[1] ? whereLine(run, details.steps, details.verifiedSha) : details.outcome[1]}
          </p>
          {/* The report's third line, counted now. */}
          <p className="text-muted-foreground text-xs">
            {details.waiting === 0
              ? "Nothing waits on you."
              : `${details.waiting} ${details.waiting === 1 ? "thing waits" : "things wait"} on you.`}
          </p>
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
              <summary className={disclosureRow}>Rules and sources</summary>
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
      {running && !started && (
        <p className="text-muted-foreground text-xs" role="status">
          {ending
            ? "Ending the run"
            : run.state === "waitingQuota"
              ? "Waiting for limits"
              : run.state === "preparing"
                ? "Preparing the worktree"
                : "Writing the plan"}
        </p>
      )}
    </AgentPlan>
  );
}

/** `atMs` as a local "07:30". */
function clockTime(atMs: number): string {
  return new Date(atMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
}

function sentenceCase(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** "6h 12m", "45m": how long a run went. */
function spanWords(ms: number): string {
  const minutes = Math.max(0, Math.round(ms / 60_000));
  const hours = Math.floor(minutes / 60);
  return hours > 0 ? `${hours}h ${minutes % 60}m` : `${minutes}m`;
}

/** The plan's own phase a step carries out: its scope and "done when". */
function sourcePhase(run: OvernightRun, step: RunStep): OvernightPhase | undefined {
  return run.phases.find((phase) => phase.number === step.number);
}

/**
 * A started run's progress: the phase at work and how many are done, settling into how the run
 * came out.
 */
function RunProgress({ model, over, ending }: { model: OvernightCardModel; over: boolean; ending: boolean }) {
  const { run, details } = model;
  const { done, worked } = phasesDone(details.steps);
  const counted = `${done} of ${worked} done`;
  if (over) {
    const status: JobOutcomeStatus =
      done === worked ? "success" : done > 0 ? "partial" : run.stop?.type === "stopped" ? "cancelled" : "failed";
    const took =
      run.startedAtMs !== null && run.finishedAtMs !== null ? spanWords(run.finishedAtMs - run.startedAtMs) : undefined;
    return <JobProgress title={counted} done={done} total={worked} meta={took} outcome={{ status }} />;
  }
  const current = details.steps.find((step) => step.mark === "working");
  const title = ending
    ? "Ending the run"
    : run.state === "preparing"
      ? "Preparing the worktree"
      : current
        ? phaseTitle(current, details.steps.length)
        : "Starting the next phase";
  return (
    <JobProgress
      title={title}
      done={done}
      total={worked}
      meta={counted}
      description={run.state === "waitingQuota" ? "Waiting for limits" : undefined}
    />
  );
}

/** A phase on the run's timeline: when it ran, how it came out, and its work behind a disclosure. */
function phaseEvent(
  run: OvernightRun,
  step: RunStep,
  over: boolean,
  quota: string | undefined,
  taskIds: readonly string[],
): TimelineEvent {
  const now = !over && step.mark === "working";
  const future = step.mark === "pending" || (step.mark === "skipped" && !over);
  const at = now ? step.startedAtMs : (step.endedAtMs ?? step.startedAtMs);
  return {
    id: `phase-${step.number}`,
    when: now ? "now" : future ? "future" : "past",
    time: at !== null && !future ? clockTime(at) : "",
    title: `Phase ${step.number} · ${step.name}`,
    detail: quota || phaseOutcome(run, step, over) || sentenceCase(phaseWord(step.mark, over)),
    more: <PhaseMore phase={sourcePhase(run, step)} open={now} taskIds={taskIds} />,
  };
}

/** A phase's workers and what it covers; open while the phase is at work. */
function PhaseMore({
  phase,
  open,
  taskIds,
}: {
  phase: OvernightPhase | undefined;
  open: boolean;
  taskIds: readonly string[];
}) {
  const covers = phase && (phase.scope || phase.doneWhen.length > 0) && (
    <details>
      <summary className={cn(disclosureRow, "text-xs")}>What it covers</summary>
      <div className="text-muted-foreground flex flex-col gap-1.5 pt-1 text-xs">
        {phase.scope && <p>{phase.scope}</p>}
        <DoneWhen lines={phase.doneWhen} />
      </div>
    </details>
  );
  if (taskIds.length === 0) return covers || null;
  const work = <PhaseWork taskIds={taskIds} />;
  return (
    <div className="text-muted-foreground flex flex-col gap-1 pt-1 text-xs">
      {open ? (
        work
      ) : (
        <details>
          <summary className={disclosureRow}>Workers</summary>
          <div className="pt-1">{work}</div>
        </details>
      )}
      {covers}
    </div>
  );
}

/** A phase's "done when" lines. */
function DoneWhen({ lines }: { lines: readonly { id: string; text: string }[] }) {
  if (lines.length === 0) return null;
  return (
    <div>
      <p className="font-medium">Done when</p>
      <ul className="list-inside list-disc space-y-1">
        {lines.map((line) => (
          <li key={line.id}>{line.text}</li>
        ))}
      </ul>
    </div>
  );
}
