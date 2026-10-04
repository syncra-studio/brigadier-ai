import { memo, useId, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { Lines } from "@/app/conversation/cards/common";
import { earlierPlans } from "@/app/conversation/cards/planHistory";
import { planProgress, planStepStatus } from "@/app/conversation/planProgress";
import { taskState } from "@/app/conversation/rowWords";
import { WorkerChip } from "@/app/conversation/WorkerChip";
import { useAction } from "@/app/conversation/useAction";
import { type AgentPlanStepStatus, StepMark } from "@/components/assistant-ui/elements/agent-plan";
import { disclosureRow } from "@/components/assistant-ui/elements/surfaces";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { Gate, GateMember, Plan, PlanState, TaskState } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";
import { decidePlan } from "@/state/actions";

/** A plan's state in words, as its badge says it. */
function planStateWord(state: PlanState): string {
  switch (state.type) {
    case "proposed":
      return "proposed";
    case "inReview":
      return "in plan review";
    case "approved":
      return state.by === "user" ? "approved by you" : state.by === "brigadier" ? "auto-approved" : "approved after plan review";
    case "rejected":
      return "rejected";
    case "superseded":
      return "replaced by a revision";
    case "revising":
      return "being revised";
  }
}

/** The reviewers of a round that are on the board, as chips. */
function Reviewers({ members }: { members: readonly GateMember[] }) {
  const onBoard = useBoard(
    useShallow((s) => members.map((member) => Boolean(s.board?.tasks[member.taskId]))),
  );
  const shown = members.filter((_, index) => onBoard[index]);
  if (shown.length === 0) return "the reviewer";
  return shown.map((member, index) => (
    <span key={member.taskId}>
      {index > 0 && " and "}
      <WorkerChip taskId={member.taskId} label="Review" />
    </span>
  ));
}

function count(n: number, word: string): string {
  return `${n} ${word}${n === 1 ? "" : "s"}`;
}

/**
 * The plan's independent review: how the revision answered the earlier round's findings, and
 * the current round with its findings or notes.
 */
function PlanReview({ plan }: { plan: Plan }) {
  // The round before, on the plan this one revises.
  const before = useBoard((s) =>
    plan.revises ? (s.board?.plans[plan.revises]?.gate ?? null) : null,
  );
  const declined = plan.responses.filter((response) => !response.accepted);
  return (
    <>
      {before && plan.responses.length > 0 && (
        <div className="flex flex-col gap-1">
          <p className="text-muted-foreground text-xs">
            Round {before.round} reviewed by <Reviewers members={before.members} />:{" "}
            {count(plan.responses.length, "finding")}, {plan.responses.length - declined.length}{" "}
            accepted, {declined.length} declined.
          </p>
          {declined.length > 0 && (
            <Lines
              items={declined.map(
                (response) =>
                  `${response.id} declined: ${response.finding} (why: ${response.note})`,
              )}
            />
          )}
        </div>
      )}
      {plan.gate && <ReviewRound gate={plan.gate} notes={plan.reviewNotes} />}
    </>
  );
}

function ReviewRound({ gate, notes }: { gate: Gate; notes: readonly string[] }) {
  const outcome = gate.outcome?.type ?? null;
  // A round cut short by a decision or a newer plan shows only what it found by then.
  if (outcome === "superseded" && gate.findings.length === 0) return null;
  const reasons = gate.members.flatMap((member) =>
    member.result?.type === "noResult" || member.result?.type === "unverified"
      ? [member.result.reason]
      : [],
  );
  // Findings show as each reviewer's result arrives, whatever the round's outcome.
  const findings = gate.findings.map((finding) => `${finding.id}: ${finding.text}`);
  return (
    <div className="flex flex-col gap-1">
      <p className={cn("text-muted-foreground text-xs", outcome === null && "shimmer")}>
        Round {gate.round}:{" "}
        {outcome === null ? (
          <>
            in review by <Reviewers members={gate.members} />
            {findings.length > 0 && `, ${count(findings.length, "problem")} found so far`}
          </>
        ) : outcome === "passed" ? (
          <>
            approved by <Reviewers members={gate.members} />
            {notes.length > 0 && `, with ${count(notes.length, "note")}`}
          </>
        ) : outcome === "failed" ? (
          <>
            <Reviewers members={gate.members} /> found {count(findings.length, "problem")}
          </>
        ) : outcome === "superseded" ? (
          `stopped early, after finding ${count(findings.length, "problem")}`
        ) : (
          <>
            the review could not finish
            {findings.length > 0 && `; it found ${count(findings.length, "problem")} first`}
          </>
        )}
      </p>
      {outcome === "passed" && notes.length > 0 && <Lines items={notes} />}
      {findings.length > 0 && <Lines items={findings} />}
      {(outcome === "noResult" || outcome === "unverified") && reasons.length > 0 && (
        <Lines items={reasons} />
      )}
    </div>
  );
}

/** How the plan's review reads on its disclosure, before it opens. */
function reviewWord(gate: Gate | null): string {
  const round = gate?.round;
  if (!round) return "Review findings from the previous round";
  switch (gate?.outcome?.type) {
    case undefined:
      return `Review round ${round}: in progress`;
    case "passed":
      return `Approved after ${count(round, "review")}`;
    case "failed":
      return `Review round ${round}: changes requested`;
    case "superseded":
      return `Review round ${round}: stopped early`;
    case "noResult":
    case "unverified":
      return `Review round ${round}: could not finish`;
  }
}

/** A step of the plan on one line, its mark and title; it opens to its state, worker and words. */
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
      <div className="flex min-w-0 items-start gap-2">
        <button
          type="button"
          title={title}
          aria-expanded={open}
          aria-controls={id}
          onClick={() => setOpen(!open)}
          className={cn(disclosureRow, "flex min-w-0 flex-1 items-start gap-2 py-0.5 text-start text-sm")}
        >
          <StepMark status={status} />
          <span className={cn("min-w-0 flex-1", open ? "wrap-anywhere" : "truncate")}>{title}</span>
        </button>
        {taskId && <WorkerChip taskId={taskId} className="max-w-32" />}
      </div>
      <div id={id} hidden={!open} className="flex gap-2 pb-1">
        {/* Under the title, past the step's mark. */}
        <span aria-hidden className="w-icon-md shrink-0" />
        <div className="text-muted-foreground flex min-w-0 flex-1 flex-col gap-1 text-xs">
          <span>{word}</span>
          {detail && <p className="whitespace-pre-wrap wrap-anywhere">{detail}</p>}
        </div>
      </div>
    </li>
  );
}

/**
 * The session's plans before the current one, each with its earlier revisions, and the current
 * one's earlier revisions: each its title, state, steps and review, behind one disclosure.
 */
function EarlierPlans({ plan, older }: { plan: Plan; older: readonly string[] }) {
  const earlier = useBoard(useShallow((s) => earlierPlans(s.board?.plans ?? {}, plan, older)));
  if (earlier.length === 0) return null;
  return (
    <details className="text-muted-foreground text-xs">
      <summary className={disclosureRow}>Earlier plans ({earlier.length})</summary>
      <div className="flex flex-col gap-3 pt-2">
        {earlier.map((before) => (
          <div
            key={before.id}
            id={`plan-${before.id}`}
            tabIndex={-1}
            className="rounded-control flex flex-col gap-1 outline-none wrap-anywhere focus-visible:ring-1 focus-visible:ring-ring"
          >
            <p>
              <span className="text-foreground/80">{before.title}</span> · {planStateWord(before.state)}
            </p>
            <ol className="list-inside list-decimal">
              {before.steps.map((step, index) => (
                <li key={index}>{step.title}</li>
              ))}
            </ol>
            {before.gate && <ReviewRound gate={before.gate} notes={before.reviewNotes} />}
          </div>
        ))}
      </div>
    </details>
  );
}

/**
 * The session's plan, a section of the summary's context card: one line per step (each opening
 * to its state, worker and description), the review behind a disclosure, the approval while it
 * is proposed, and earlier plans behind another. `planIds` are the session's own plans, oldest
 * first; `currentPlanId` selects the active request's newest plan.
 */
export const PlanSection = memo(function PlanSection({
  planIds,
  currentPlanId,
}: {
  planIds: readonly string[];
  currentPlanId?: string;
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
  const reviewGate = useBoard((s) =>
    plan?.gate ?? (plan?.revises ? s.board?.plans[plan.revises]?.gate ?? null : null),
  );
  // Who decides a proposed plan: the user under Ask for approval or in plan mode.
  const decider = useApp((s) => {
    const conversation = plan ? s.conversations[plan.conversationId] : null;
    const setup = conversation?.lifecycle === "archived" ? null : conversation?.setup;
    if (setup?.type !== "session") return null;
    return setup.permission === "askForApproval" || setup.planMode ? "user" : "brigadier";
  });
  if (!plan) return null;

  const progress = planProgress(
    plan,
    plan.steps.map((_, index) => (steps[index * 3 + 1] ?? undefined) as TaskState | undefined),
  );
  const variant = progress.tone === "failed" ? "destructive"
    : progress.tone === "done" ? "success"
    : progress.tone === "warning" ? "warning" : "secondary";
  const proposed = plan.state.type === "proposed";
  const reviewed = plan.gate !== null || plan.responses.length > 0;

  return (
    <section
      data-card="plan"
      id={`plan-${plan.id}`}
      tabIndex={-1}
      aria-label={`Plan: ${plan.title}`}
      className="rounded-control flex flex-col gap-1 outline-none focus-visible:ring-1 focus-visible:ring-ring"
    >
      <h3
        title={plan.title}
        className="text-muted-foreground flex min-h-control-xs flex-wrap items-start justify-between gap-2 text-xs"
      >
        Plan
        <span className="flex min-w-0 flex-1 flex-wrap justify-end gap-1.5">
          {plan.risky && (
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  type="button"
                  aria-label="Why this plan is risky"
                  className="rounded-capsule outline-none focus-visible:ring-1 focus-visible:ring-ring"
                >
                  <Badge variant="warning">Risky</Badge>
                </button>
              </TooltipTrigger>
              <TooltipContent>Risky plans get two independent reviewers.</TooltipContent>
            </Tooltip>
          )}
          <Badge variant={variant} className="h-auto min-h-pill max-w-full whitespace-normal text-start wrap-anywhere">
            {progress.label}
          </Badge>
        </span>
      </h3>
      {plan.state.type === "approved" && (
        <p className="text-muted-foreground text-xs" data-auto-approved={plan.state.by === "brigadier" || undefined}>
          {plan.state.by === "user" ? "Approved by you" : plan.state.by === "brigadier"
            ? plan.reviewSkipReason ? `Approved without review: ${plan.reviewSkipReason}` : "Auto-approved by Brigadier"
            : "Approved after plan review"}
        </p>
      )}
      <ol className="flex flex-col">
        {plan.steps.map((step, index) => {
          const taskId = steps[index * 3] as string | null | undefined;
          const state = steps[index * 3 + 1] as TaskState | null | undefined;
          const word = steps[index * 3 + 2] as string | null | undefined;
          return (
            <PlanStepRow
              key={index}
              title={step.title}
              detail={step.detail}
              status={planStepStatus(state ?? undefined)}
              // The same words as the worker's row in the thread.
              word={word ?? "Not started"}
              taskId={taskId ?? null}
            />
          );
        })}
      </ol>
      {reviewed && (
        <details className="text-muted-foreground text-xs">
          <summary className={disclosureRow}>{reviewWord(reviewGate)}</summary>
          <div className="flex flex-col gap-2 pt-1">
            <PlanReview plan={plan} />
          </div>
        </details>
      )}
      {plan.state.type === "rejected" && plan.state.message && (
        <p className="text-muted-foreground text-xs">Rejected: {plan.state.message}</p>
      )}
      {proposed && decider === "user" && <PlanDecision plan={plan} />}
      {proposed && decider === "brigadier" && (
        <p className="text-muted-foreground text-xs">
          Brigadier decides this plan for you
          {plan.risky || plan.steps.length > 1 ? " after an independent review" : ""}.
        </p>
      )}
      <EarlierPlans plan={plan} older={planIds.filter((id) => id !== currentId)} />
    </section>
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
