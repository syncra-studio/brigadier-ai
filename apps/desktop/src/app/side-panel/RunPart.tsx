import { DoneWhen } from "@/app/conversation/cards/DoneWhen";
import { PhaseMore } from "@/app/conversation/cards/PhaseMore";
import { RulesAndSources } from "@/app/conversation/cards/RulesAndSources";
import {
  type OvernightActions,
  type OvernightCardModel,
  restrictionLines,
} from "@/app/conversation/overnightAdapter";
import { phaseOutcome, phaseWord, type RunStep, runOver } from "@/app/conversation/phaseView";
import { runJob } from "@/app/conversation/runJob";
import { useRunCommand } from "@/app/conversation/useRunCommand";
import { MarkdownBlock, Timeline, type TimelineEvent } from "@/components/conversation";
import { Button } from "@/components/controls";

/** `atMs` as a local "07:30". */
function clockTime(atMs: number): string {
  return new Date(atMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
}

function sentenceCase(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** Where the run stands, after its name. */
function stateWords(model: OvernightCardModel): string {
  const { run } = model;
  if (run.state === "proposed") return "Not started";
  if (run.state === "windingDown" || run.state === "reporting") return "Writing the report";
  if (!runOver(run)) return "At work";
  switch (runJob(model).outcome?.status) {
    case "success":
      return "Done";
    case "failed":
      return "Failed";
    case "cancelled":
      return "Stopped";
    default:
      return "Partly done";
  }
}

/** A phase on the run's timeline: when it ran, how it came out, and what it covers. */
function phaseEvent(model: OvernightCardModel, step: RunStep): TimelineEvent {
  const { run, details } = model;
  const over = runOver(run);
  const phase = run.phases.find((item) => item.number === step.number);
  const progress = details.phaseProgress[step.number];
  const quota =
    progress?.quota &&
    `Waiting for ${progress.quota.provider ? `${progress.quota.provider} ` : ""}limits · resets ${clockTime(progress.quota.resetsAtMs)}`;
  const now = !over && step.mark === "working";
  const future = step.mark === "pending" || (step.mark === "skipped" && !over);
  const at = now ? step.startedAtMs : (step.endedAtMs ?? step.startedAtMs);
  return {
    id: `phase-${step.number}`,
    when: now ? "now" : future ? "future" : "past",
    time: at !== null && !future ? clockTime(at) : "",
    title: `Phase ${step.number} · ${step.name}`,
    detail:
      run.state === "proposed"
        ? step.mark === "skipped" ? "Skipped, as the run's restrictions say." : undefined
        : quota || phaseOutcome(run, step, over) || sentenceCase(phaseWord(step.mark, over)),
    // Before the start, what each phase covers is the plan the user agrees to: all of it shows.
    more:
      run.state === "proposed" ? (
        phase && (phase.scope || phase.doneWhen.length > 0) ? (
          <div data-slot="run-phase-scope" className="text-muted-foreground flex flex-col gap-1.5 pt-1 text-xs">
            {phase.scope && <p>{phase.scope}</p>}
            <DoneWhen lines={phase.doneWhen} />
          </div>
        ) : undefined
      ) : (
        <PhaseMore phase={phase} open={now} taskIds={progress?.workerTaskIds ?? []} />
      ),
    failed: step.mark === "blocked",
  };
}

/** Before the start: when the report is due, the rules, the restrictions and what stands in the way. */
function RunTerms({ model, onLid }: { model: OvernightCardModel; onLid: (() => void) | null }) {
  const { run, details } = model;
  const power = details.power;
  const deadline = run.directives.deadline;
  return (
    <div className="flex flex-col gap-1 text-xs">
      {deadline.type === "at" && (
        <p className="text-muted-foreground">
          Report ready {deadline.time.day} · {deadline.time.localTime} ({deadline.time.timeZone})
        </p>
      )}
      {(run.rules || run.sources.length > 0) && <RulesAndSources rules={run.rules} sources={run.sources} />}
      {restrictionLines(run).map((line) => (
        <p key={line}>{line}</p>
      ))}
      {/* Each line already reads "Ignored: … (Brigadier picks this itself)". */}
      {run.directives.ignored.map((line) => (
        <p key={line} className="text-muted-foreground">
          {line}
        </p>
      ))}
      {run.problems.map((problem, index) => (
        <p key={index} role="alert" className="text-warning">
          {problem.message}
        </p>
      ))}
      {power?.onBattery && !power.lowBattery && <p className="text-warning">On battery: plug in to be safe</p>}
      {power?.lidWillPause && (
        <p className="text-warning">
          {power.lowBattery ? "The battery is low: closing the lid will pause the run. Plug it in." : "Closing the lid will pause the run"}
        </p>
      )}
      {power?.lidWillPause && power.offerLidSetup && onLid && (
        <Button type="button" variant="link-blue" size="xs" className="self-start px-0" onClick={onLid}>
          Keep running with the lid closed
        </Button>
      )}
    </div>
  );
}

/** What the run came to: its report in full (or its outcome lines until the report loads). */
function RunReport({ model }: { model: OvernightCardModel }) {
  const { details } = model;
  if (details.report) {
    return (
      <div data-slot="run-report" className="text-sm">
        <MarkdownBlock text={details.report} />
      </div>
    );
  }
  if (!details.outcome) return null;
  return (
    <div data-slot="run-report" className="flex flex-col gap-1 text-sm">
      {details.outcome.map((line) => (
        <p key={line}>{line}</p>
      ))}
    </div>
  );
}

/**
 * An overnight run's part of the job's tab, opened from its Job card (and its Edit): before the
 * start, its terms and each phase with its scope and "done when"; while it works, its phases on a
 * timeline with their workers; once over, its report, then Merge and Continue.
 */
export function RunPart({ model, actions }: { model: OvernightCardModel; actions: OvernightActions | null }) {
  const { run, details } = model;
  const command = useRunCommand(run);
  const proposed = run.state === "proposed";
  const over = runOver(run);
  const sha = details.verifiedSha;
  const merged = !!sha && run.merged?.verifiedCommit === sha;
  return (
    <section
      id={`overnight-${run.id}`}
      tabIndex={-1}
      data-slot="run-part"
      data-run-state={run.state}
      className="flex scroll-mt-4 flex-col gap-4 outline-none"
    >
      <h2 className="text-muted-foreground text-label">
        Overnight · <span className="text-foreground">{run.name}</span> · {stateWords(model)}
      </h2>
      {proposed && <RunTerms model={model} onLid={actions ? () => command.send("lid", actions.setUpLidClosed) : null} />}
      {!proposed && !over && (
        <p className="text-muted-foreground text-xs tabular-nums" role="status">
          {details.waiting} waiting on you · {details.decided} decided for you
        </p>
      )}
      {over && <RunReport model={model} />}
      {over && actions && (sha || details.reportMessageId || details.remaining > 0) && (
        <div className="flex flex-wrap items-center gap-2">
          {sha && (
            <Button
              type="button"
              size="xs"
              disabled={command.busy || merged}
              onClick={() => command.send("merge", (sent) => actions.merge(sent, sha))}
            >
              {merged ? "Merged" : "Merge"}
            </Button>
          )}
          {details.remaining > 0 && (
            <Button
              type="button"
              size="xs"
              variant="secondary"
              disabled={command.busy}
              onClick={() => command.send("continue", (sent) => actions.continue(sent, "continue until done"))}
            >
              Continue
            </Button>
          )}
          {details.reportMessageId && (
            <Button
              type="button"
              size="xs"
              variant="link-blue"
              onClick={() => actions.openReport(run.conversationId, details.reportMessageId!)}
            >
              Show in the thread
            </Button>
          )}
        </div>
      )}
      {over && run.notification?.deliveryError && !details.notificationsOff && (
        <p role="status" className="text-muted-foreground text-xs">
          The notification couldn’t be shown: {run.notification.deliveryError}
        </p>
      )}
      {details.notificationsOff && (
        <p className="text-warning text-xs" role="status">
          {over
            ? "Notifications are off for Brigadier, so the run’s notification wasn’t shown. "
            : "Notifications are off for Brigadier, so you won’t hear when the run finishes. "}
          {actions && (
            <Button
              type="button"
              variant="link-blue"
              size="xs"
              className="h-auto px-0 align-baseline"
              disabled={command.busy}
              onClick={() => command.send("notifications", actions.openNotificationSettings)}
            >
              Turn them on
            </Button>
          )}
        </p>
      )}
      {details.steps.length > 0 ? (
        <Timeline aria-label="Phases" events={details.steps.map((step) => phaseEvent(model, step))} />
      ) : (
        // A bare goal: the run's thread writes the plan first.
        <p className="text-muted-foreground text-xs">
          {proposed ? "The run writes its plan first, for this goal: " : "Writing the plan for this goal: "}
          {run.goal}
        </p>
      )}
      {command.error && (
        <p role="alert" className="text-destructive text-xs">
          {command.error}
        </p>
      )}
    </section>
  );
}

