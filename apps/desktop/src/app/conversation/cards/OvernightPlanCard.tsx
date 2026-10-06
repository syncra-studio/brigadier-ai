import { Moon } from "@openai/apps-sdk-ui/components/Icon";
import { useContext, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { phaseOutcome, phaseWord, runOver } from "@/app/conversation/phaseView";
import { plainLine, taskState } from "@/app/conversation/rowWords";
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
import type { OvernightPhase, OvernightRun, PhaseState, Plan } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";

const MARK: Record<PhaseState, AgentPlanStepStatus> = {
  pending: "pending",
  running: "active",
  checking: "active",
  verified: "done",
  partial: "partial",
  blocked: "failed",
  skipped: "skipped",
};

/** The newest plan a phase's lead wrote (or Phase 0's), which the run card shows inside the phase. */
function usePhasePlan(requestId: string | null, planId: string | null): Plan | undefined {
  return useBoard((s) => {
    const plans = Object.values(s.board?.plans ?? {});
    const own = planId ? s.board?.plans[planId] : undefined;
    if (own) return own;
    return plans
      .filter((plan) => requestId !== null && plan.requestId === requestId && plan.state.type !== "superseded")
      .toSorted((a, b) => b.createdAtMs - a.createdAtMs)[0];
  });
}

const NUL = "\u0000";

/**
 * A phase's work inside the run card: one line per step of its plan (or per worker, without a
 * plan), each opening its worker, then the plan's step details behind a disclosure.
 */
function PhaseWork({
  runId,
  phaseId,
  requestId,
  planId = null,
}: {
  runId: string;
  phaseId: string;
  requestId: string | null;
  planId?: string | null;
}) {
  const { setPanel } = useContext(AgentsPanelContext);
  const plan = usePhasePlan(requestId, planId);
  // Each line as "task id NUL state NUL title", so the card re-renders only when one changes.
  const lines = useBoard(
    useShallow((s) => {
      const tasks = s.board?.tasks ?? {};
      if (plan && plan.steps.length > 0) {
        return plan.steps.map((step) => {
          const task = step.taskId ? tasks[step.taskId] : undefined;
          return [task?.id ?? "", task ? taskState(task).word : "planned", step.title].join(NUL);
        });
      }
      return Object.values(tasks)
        // An older run's whole-phase checks list here too; its other checks open from what they checked.
        .filter((task) => (task.gateLink === null || task.gateLink.owner.type === "phase") && task.run?.runId === runId && task.run.phaseId === phaseId)
        .toSorted((a, b) => a.number - b.number)
        .map((task) => [task.id, taskState(task).word, task.title].join(NUL));
    }),
  );
  const details = plan?.steps.filter((step) => step.detail) ?? [];
  return (
    <div className="flex flex-col gap-1.5 whitespace-normal">
      {lines.length > 0 && (
        <ul className="flex flex-col">
          {lines.map((line, index) => {
            const [taskId = "", word = "", title = ""] = line.split(NUL);
            const text = (
              <>
                <span className="text-foreground/80 min-w-0 truncate">{title}</span>
                <span className="shrink-0">· {word}</span>
              </>
            );
            return (
              <li key={`${index}:${taskId}`} className="flex min-w-0">
                {taskId ? (
                  <button
                    type="button"
                    title={title}
                    onClick={() => setPanel(taskId)}
                    className="hover:text-foreground rounded-control flex min-w-0 items-center gap-1 text-start"
                  >
                    {text}
                  </button>
                ) : (
                  <span className="flex min-w-0 items-center gap-1" title={title}>
                    {text}
                  </span>
                )}
              </li>
            );
          })}
        </ul>
      )}
      {details.length > 0 && (
        <details>
          <summary className={disclosureRow}>
            Step details
          </summary>
          <div className="flex flex-col gap-1.5 pt-1 wrap-anywhere">
            {details.map((step) => (
              <p key={step.title}>
                <span className="text-foreground/80">{step.title}:</span> {plainLine(step.detail ?? "")}
              </p>
            ))}
          </div>
        </details>
      )}
    </div>
  );
}

/** Which verified phases Merge takes: "phase 1", "phases 1 and 2". */
function mergedPhases(run: OvernightRun): string | null {
  const numbers = run.phases.filter((phase) => phase.state === "verified").map((phase) => phase.number);
  if (numbers.length === 0) return null;
  if (numbers.length === 1) return `phase ${numbers[0]}`;
  return `phases ${numbers.slice(0, -1).join(", ")} and ${numbers.at(-1)}`;
}

/** The finished card's second line: what Merge takes, and where the work is. */
function whereLine(run: OvernightRun, verifiedSha: string | undefined): string {
  const branch = run.workspace ? `on ${run.workspace.branch}` : "";
  const phases = mergedPhases(run);
  if (run.merged) return `Merged into ${run.workspace?.base ?? "the base branch"} · ${branch}`;
  if (!verifiedSha || !phases) return `Nothing verified to merge yet · ${branch}`;
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
  const started = !proposed && run.phases.length > 0;
  const quotaWord = (phase: OvernightPhase) => {
    const quota = details.phaseProgress[phase.id]?.quota;
    return quota && `Waiting for ${quota.provider} limits · resets ${clockTime(quota.resetsAtMs)}`;
  };
  const steps: AgentPlanStep[] = (started ? [] : run.phases).map((phase) => {
    const progress = details.phaseProgress[phase.id];
    const workerTaskIds = progress?.workerTaskIds;
    const status = quotaWord(phase) || phaseWord(phase.state, over);
    return {
      key: phase.id,
      title: `Phase ${phase.number} · ${phase.name}`,
      // A phase the run's end cut off is unfinished, not still at work.
      status: over && MARK[phase.state] === "active" ? "partial" : MARK[phase.state],
      statusLabel: proposed ? "" : status,
      live: running,
      folded: proposed || (phase.state !== "running" && phase.state !== "checking"),
      detail: proposed ? (
        <div className="flex flex-col gap-1.5">
          {phase.scope && <p>{phase.scope}</p>}
          <DoneWhen criteria={phase.doneWhen} />
        </div>
      ) : (
        <div className="flex flex-col gap-1.5">
          {Boolean(workerTaskIds?.length) && (
            <PhaseWork runId={run.id} phaseId={phase.id} requestId={phase.requestId} />
          )}
          {(phase.scope || phase.doneWhen.length > 0) && (
            <details>
              <summary className={disclosureRow}>
                What it covers
              </summary>
              <div className="flex flex-col gap-1.5 pt-1">
                {phase.scope && <p>{phase.scope}</p>}
                <DoneWhen
                  criteria={phase.doneWhen}
                  results={progress?.criteria}
                />
              </div>
            </details>
          )}
        </div>
      ),
    };
  });
  if (!started && steps.length === 0)
    steps.push({
      key: "phase-0",
      title: "Phase 0 · Write the plan",
      detail: run.planning ? (
        <div className="flex flex-col gap-1.5">
          <PhaseWork
            runId={run.id}
            phaseId="phase-0"
            requestId={run.planning.requestId}
            planId={run.planning.planId}
          />
          <p>{run.goal}</p>
        </div>
      ) : (
        run.goal
      ),
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
            events={run.phases.map((phase) =>
              phaseEvent(run, phase, over, quotaWord(phase), details.phaseProgress[phase.id]?.criteria),
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
            {run.merged || !details.outcome?.[1] ? whereLine(run, details.verifiedSha) : details.outcome[1]}
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
                : run.state === "planning"
                  ? "Writing the plan"
                  : "Working through the plan"}
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

/**
 * A started run's progress: the phase at work and how many are verified, settling into how the
 * run came out.
 */
function RunProgress({ model, over, ending }: { model: OvernightCardModel; over: boolean; ending: boolean }) {
  const { run } = model;
  const selected = run.phases.filter((phase) => phase.state !== "skipped");
  const verified = selected.filter((phase) => phase.state === "verified").length;
  const counted = `${verified} of ${selected.length} verified`;
  if (over) {
    const status: JobOutcomeStatus =
      verified === selected.length ? "success" : verified > 0 ? "partial" : run.stop?.type === "stopped" ? "cancelled" : "failed";
    const took =
      run.startedAtMs !== null && run.finishedAtMs !== null ? spanWords(run.finishedAtMs - run.startedAtMs) : undefined;
    return (
      <JobProgress
        title={`${counted}`}
        done={verified}
        total={selected.length}
        meta={took}
        outcome={{ status }}
      />
    );
  }
  const index = selected.findIndex((phase) => phase.state === "running" || phase.state === "checking");
  const current = index >= 0 ? selected[index] : undefined;
  const title = ending
    ? "Ending the run"
    : run.state === "preparing"
      ? "Preparing the worktree"
      : current
        ? `Phase ${index + 1} of ${selected.length} · ${current.name}`
        : "Starting the next phase";
  return (
    <JobProgress
      title={title}
      done={verified}
      total={selected.length}
      meta={counted}
      description={run.state === "waitingQuota" ? "Waiting for limits" : undefined}
    />
  );
}

/** A phase on the run's timeline: when it ran, how it came out, and its work behind a disclosure. */
function phaseEvent(
  run: OvernightRun,
  phase: OvernightPhase,
  over: boolean,
  quota: string | undefined,
  results: Readonly<Record<string, string>> | undefined,
): TimelineEvent {
  const now = !over && (phase.state === "running" || phase.state === "checking");
  const future = phase.state === "pending" || (phase.state === "skipped" && !over);
  const at = now ? phase.startedAtMs : (phase.settledAtMs ?? phase.startedAtMs);
  return {
    id: phase.id,
    when: now ? "now" : future ? "future" : "past",
    time: at !== null && !future ? clockTime(at) : "",
    title: `Phase ${phase.number} · ${phase.name}`,
    detail: quota || phaseOutcome(run, phase, over) || sentenceCase(phaseWord(phase.state, over)),
    more: <PhaseMore run={run} phase={phase} open={now} results={results} />,
  };
}

/** A phase's workers and what it covers; open while the phase is at work. */
function PhaseMore({
  run,
  phase,
  open,
  results,
}: {
  run: OvernightRun;
  phase: OvernightPhase;
  open: boolean;
  /** An older run's checks of each criterion. */
  results: Readonly<Record<string, string>> | undefined;
}) {
  const covers = (phase.scope || phase.doneWhen.length > 0) && (
    <details>
      <summary className={cn(disclosureRow, "text-xs")}>What it covers</summary>
      <div className="text-muted-foreground flex flex-col gap-1.5 pt-1 text-xs">
        {phase.scope && <p>{phase.scope}</p>}
        <DoneWhen criteria={phase.doneWhen} results={results} />
      </div>
    </details>
  );
  if (phase.requestId === null) return covers || null;
  const work = <PhaseWork runId={run.id} phaseId={phase.id} requestId={phase.requestId} />;
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

/** A phase's done-when criteria, each with what its checks found once they ran. */
function DoneWhen({
  criteria,
  results,
}: {
  criteria: readonly { id: string; text: string }[];
  results?: Readonly<Record<string, string>> | undefined;
}) {
  if (criteria.length === 0) return null;
  return (
    <div>
      <p className="font-medium">Done when</p>
      <ul className="list-inside list-disc space-y-1">
        {criteria.map((criterion) => (
          <li key={criterion.id}>
            {criterion.text}
            {results?.[criterion.id] && ` · ${results[criterion.id]}`}
          </li>
        ))}
      </ul>
    </div>
  );
}
