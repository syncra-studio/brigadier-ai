import { Lightbulb } from "@openai/apps-sdk-ui/components/Icon";
import { memo, useContext, useId, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { planStepStatus, planProgress, stepStates } from "@/app/conversation/planProgress";
import { SidePanelContext } from "@/app/conversation/SidePanel";
import { taskState } from "@/app/conversation/rowWords";
import { WorkerChip } from "@/app/conversation/WorkerChip";
import { useAction } from "@/app/conversation/useAction";
import { type AgentPlanStepStatus, StepMark } from "@/components/assistant-ui/elements/agent-plan";
import { SummarySection, summaryRowInteractive } from "@/components/assistant-ui/elements/summary-section";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { Plan, TaskState } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";
import { showPlanDoc } from "@/state/planDoc";
import { useApp } from "@/state/store";
import { decidePlan } from "@/state/actions";

/** A step's state at the end of its row, as the worker's own row would say it in short. */
const STEP_WORDS: Record<AgentPlanStepStatus, string> = {
  pending: "Pending",
  active: "In progress",
  done: "Done",
  partial: "Partly done",
  failed: "Failed",
  skipped: "Skipped",
};

/** A step of the plan on one line, its mark, title and state; it opens to its worker and words. */
function PlanStepRow({
  title,
  detail,
  status,
  word,
  taskId,
}: {
  title: string;
  detail: string | null;
  status: AgentPlanStepStatus;
  word: string;
  taskId: string | null;
}) {
  const id = useId();
  const [open, setOpen] = useState(false);
  return (
    <li className="flex flex-col">
      <button
        type="button"
        title={title}
        aria-expanded={open}
        aria-controls={id}
        onClick={() => setOpen(!open)}
        className={cn(
          summaryRowInteractive,
          "relative isolate flex min-h-row-sm min-w-0 items-start gap-2 py-1 text-start text-label",
        )}
      >
        <StepMark status={status} className="h-(--text-label--line-height)" />
        <span
          className={cn(
            "min-w-0 flex-1",
            open ? "wrap-anywhere" : "truncate",
            status === "done" && "text-muted-foreground",
          )}
        >
          {title}
        </span>
        <span className="text-muted-foreground/70 shrink-0 text-xs leading-(--text-label--line-height)">
          {STEP_WORDS[status]}
        </span>
      </button>
      <div id={id} hidden={!open} className="flex gap-2 pb-1">
        {/* Under the title, past the step's mark. */}
        <span aria-hidden className="w-icon-md shrink-0" />
        <div className="text-muted-foreground flex min-w-0 flex-1 flex-col items-start gap-1 text-xs">
          <span>{word}</span>
          {detail && <p className="whitespace-pre-wrap wrap-anywhere">{detail}</p>}
          {taskId && <WorkerChip taskId={taskId} className="max-w-full" />}
        </div>
      </div>
    </li>
  );
}

/**
 * The plan's state in one short line under its title: its phase at work ("Phase 2 of 4"), how
 * many are done, or where its approval stands.
 */
function statusLine(plan: Plan, statuses: readonly AgentPlanStepStatus[]): string {
  if (plan.state.type !== "approved") return planProgress(plan, []).label;
  const failed = statuses.indexOf("failed");
  if (failed !== -1) return `Phase ${failed + 1} failed`;
  const running = statuses.indexOf("active");
  if (running !== -1) return `Phase ${running + 1} of ${statuses.length}`;
  const done = statuses.filter((status) => status === "done").length;
  return done > 0 ? `${done} of ${statuses.length} done` : "Not started";
}

/**
 * The session's plan, a section of the summary's context card: its title and state, one row per
 * phase marked Pending, In progress or Done (each opening to its worker and description), and
 * the approval while it is proposed. It folds itself once every phase is done. `planIds` are the
 * session's own plans, oldest first; `currentPlanId` selects the plan to show, and
 * `onShowCurrent`, given while that is an earlier plan, goes back to the current one.
 */
export const PlanSection = memo(function PlanSection({
  planIds,
  currentPlanId,
  onShowCurrent,
}: {
  planIds: readonly string[];
  currentPlanId?: string;
  onShowCurrent?: (() => void) | undefined;
}) {
  const currentId = currentPlanId ?? planIds.at(-1);
  const plan = useBoard((s) => (currentId ? s.board?.plans[currentId] : undefined));
  // Only what the steps show of their tasks, so unrelated task updates don't rerender the plan.
  const steps = useBoard(
    useShallow((s) =>
      (plan?.steps ?? []).flatMap((step) => {
        const task = step.taskId ? s.board?.tasks[step.taskId] : undefined;
        return [task?.id ?? null, task?.state ?? null, task ? taskState(task).word : null];
      }),
    ),
  );
  // Who decides a proposed plan: the user under Ask for approval or in plan mode.
  const decider = useApp((s) => {
    const conversation = plan ? s.conversations[plan.conversationId] : null;
    const setup = conversation?.lifecycle === "archived" ? null : conversation?.setup;
    if (setup?.type !== "session") return null;
    return setup.permission === "askForApproval" || setup.planMode ? "user" : "brigadier";
  });
  const request = useBoard((s) => (plan?.requestId ? s.board?.requests[plan.requestId]?.state.type : undefined));
  const { openTab } = useContext(SidePanelContext);
  if (!plan) return null;

  const states = stepStates(
    plan,
    plan.steps.map((_, index) => (steps[index * 3 + 1] ?? undefined) as TaskState | undefined),
    request,
  );
  const statuses = states.map(planStepStatus);
  const proposed = plan.state.type === "proposed";
  const allDone =
    plan.state.type === "approved" && statuses.length > 0 && statuses.every((status) => status === "done");
  // A plan written as a document is one row that opens it; a single phase of it is the plan
  // itself, not a row of its own.
  const phaseRows = !plan.body || plan.steps.length > 1;

  return (
    <SummarySection
      foldKey="plan"
      title="Plan"
      count={phaseRows ? plan.steps.length : undefined}
      defaultFolded={allDone && !plan.body}
      data-card="plan"
      id={`plan-${plan.id}`}
      tabIndex={-1}
      aria-label={`Plan: ${plan.title}`}
      className=""
    >
      <div className="flex flex-col gap-0.5 pb-1">
        {onShowCurrent && (
          <p className="text-muted-foreground flex min-w-0 items-center gap-1.5 text-xs">
            <span>An earlier plan</span>
            <span aria-hidden>·</span>
            <button
              type="button"
              onClick={onShowCurrent}
              className="text-link rounded-control hover:underline"
            >
              Show the current plan
            </button>
          </p>
        )}
        {plan.body ? (
          // A plan written as a document opens it in the side panel's Plan tab.
          <button
            type="button"
            title={plan.title}
            onClick={() => {
              showPlanDoc(plan.conversationId, { type: "plan", id: plan.id });
              openTab("plan");
            }}
            className={cn(summaryRowInteractive, "relative isolate flex min-w-0 items-start gap-2 py-1 text-start text-label")}
          >
            <Lightbulb aria-hidden className="size-icon-md h-(--text-label--line-height) shrink-0" />
            <span className="line-clamp-2 min-w-0 flex-1 wrap-anywhere">{plan.title}</span>
          </button>
        ) : (
          <p title={plan.title} className="line-clamp-2 text-label wrap-anywhere">
            {plan.title}
          </p>
        )}
        <p
          className={cn(
            "text-xs",
            statuses.includes("failed") || plan.state.type === "rejected"
              ? "text-destructive"
              : plan.state.type === "proposed"
                ? "text-warning"
                : allDone
                  ? "text-success"
                  : "text-muted-foreground",
          )}
        >
          {statusLine(plan, statuses)}
        </p>
      </div>
      <ol hidden={!phaseRows} className="flex flex-col">
        {plan.steps.map((step, index) => {
          const taskId = steps[index * 3] as string | null | undefined;
          const word = steps[index * 3 + 2] as string | null | undefined;
          return (
            <PlanStepRow
              key={index}
              title={step.title}
              detail={step.detail}
              status={statuses[index] ?? "pending"}
              // The same words as the worker's row in the thread.
              word={word ?? "Not started"}
              taskId={taskId ?? null}
            />
          );
        })}
      </ol>
      {plan.state.type === "rejected" && plan.state.message && (
        <p className="text-muted-foreground py-1 text-xs">Rejected: {plan.state.message}</p>
      )}
      {/* A plan written as a document is decided by "Implement this plan?" in the composer's place. */}
      {proposed && plan.body && <p className="text-muted-foreground py-1 text-xs">Waiting for your answer below.</p>}
      {proposed && !plan.body && decider === "user" && (
        <div className="pt-1.5 pb-1">
          <PlanDecision plan={plan} />
        </div>
      )}
      {proposed && !plan.body && decider === "brigadier" && (
        <p className="text-muted-foreground py-1 text-xs">
          Brigadier decides this plan for you.
        </p>
      )}
    </SummarySection>
  );
});

/** The same approval command as the rail used, kept with the plan it decides. */
function PlanDecision({ plan }: { plan: Plan }) {
  const action = useAction();
  const [message, setMessage] = useState("");
  const text = message.trim();
  return (
    <form
      className="flex flex-col gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        if (!action.busy && text)
          action.run(() => decidePlan(plan.conversationId, plan.id, false, text));
      }}
    >
      <Button
        type="button"
        size="sm"
        className="self-start max-w-full whitespace-normal"
        disabled={action.busy}
        onClick={() => action.run(() => decidePlan(plan.conversationId, plan.id, true, null))}
      >
        Yes, implement this plan
      </Button>
      <Input
        aria-label="Tell Brigadier what to change in the plan"
        placeholder="No, and tell Brigadier what to change"
        value={message}
        disabled={action.busy}
        onChange={(event) => setMessage(event.target.value)}
      />
      {text && (
        <Button
          type="submit"
          variant="outline"
          size="xs"
          disabled={action.busy}
          className="self-start"
        >
          Submit changes
        </Button>
      )}
      {action.error && (
        <p role="alert" className="text-destructive text-xs">
          {action.error}
        </p>
      )}
    </form>
  );
}
