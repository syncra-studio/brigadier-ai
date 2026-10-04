import { memo, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { Lines } from "@/app/conversation/cards/common";
import { taskState } from "@/app/conversation/rowWords";
import { WorkerChip } from "@/app/conversation/WorkerChip";
import { useAction } from "@/app/conversation/useAction";
import { AgentPlan, type AgentPlanStepStatus } from "@/components/assistant-ui/elements/agent-plan";
import { disclosureRow } from "@/components/assistant-ui/elements/surfaces";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { Gate, GateMember, Plan, PlanState, TaskState } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";
import { decidePlan } from "@/state/actions";

function stepStatus(state: TaskState | undefined): AgentPlanStepStatus {
  switch (state) {
    case undefined:
    case "queued":
      return "pending";
    case "landed":
    case "done":
      return "done";
    case "failed":
    case "rejected":
    case "stopped":
      return "failed";
    default:
      return "active";
  }
}

function PlanStateBadge({ state }: { state: PlanState }) {
  switch (state.type) {
    case "proposed":
      return <Badge variant="warning">Proposed</Badge>;
    case "inReview":
      return <Badge variant="secondary">In plan review</Badge>;
    case "approved":
      return state.by === "user" ? (
        <Badge variant="success">Approved by you</Badge>
      ) : state.by === "brigadier" ? (
        <Badge variant="success" data-auto-approved>
          Auto-approved by Brigadier
        </Badge>
      ) : (
        <Badge variant="success">Approved after plan review</Badge>
      );
    case "rejected":
      return <Badge variant="destructive">Rejected</Badge>;
    case "superseded":
      return <Badge variant="outline">Superseded</Badge>;
    case "revising":
      return <Badge variant="warning">Being revised</Badge>;
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
      {plan.revises && <PlanHistory plan={plan} />}
    </>
  );
}

function PlanHistory({ plan }: { plan: Plan }) {
  const revisions = useBoard(
    useShallow((s) => {
      const history: Plan[] = [];
      const seen = new Set([plan.id]);
      let id = plan.revises;
      while (id && !seen.has(id)) {
        seen.add(id);
        const before = s.board?.plans[id];
        if (!before) break;
        history.push(before);
        id = before.revises;
      }
      return history;
    }),
  );
  if (revisions.length === 0) return null;
  return (
    <details className="text-muted-foreground text-xs">
      <summary className={disclosureRow}>
        Earlier revisions ({revisions.length})
      </summary>
      <div className="flex flex-col gap-3 pt-2">
        {revisions.map((revision) => (
          <div key={revision.id} className="flex flex-col gap-1 wrap-anywhere">
            <p className="font-medium">{revision.title}</p>
            <ol className="list-inside list-decimal">
              {revision.steps.map((step, index) => (
                <li key={index}>
                  {step.title}
                  {step.detail && ` · ${step.detail}`}
                </li>
              ))}
            </ol>
            {revision.gate && <ReviewRound gate={revision.gate} notes={revision.reviewNotes} />}
          </div>
        ))}
      </div>
    </details>
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
      {outcome === "passed" && notes.length > 0 && (
        // An approved plan's notes are for whoever carries it out: behind a disclosure.
        <details className="text-muted-foreground text-xs">
          <summary className={disclosureRow}>
            Review notes
          </summary>
          <div className="pt-1">
            <Lines items={notes} />
          </div>
        </details>
      )}
      {findings.length > 0 && <Lines items={findings} />}
      {(outcome === "noResult" || outcome === "unverified") && reasons.length > 0 && (
        <Lines items={reasons} />
      )}
    </div>
  );
}

/** The orchestrator's plan: its steps with the tasks carrying them out, and its approval. */
export const PlanCardView = memo(function PlanCardView({
  cardId,
  className,
}: {
  cardId: string;
  /** For the card's own surface, e.g. none inside the summary's card. */
  className?: string;
}) {
  const plan = useBoard((s) => s.board?.plans[cardId]);
  // Only what the steps show of their tasks, so unrelated task updates don't rerender the plan.
  const steps = useBoard(
    useShallow((s) =>
      (plan?.steps ?? []).flatMap((step) => {
        const task = step.taskId ? s.board?.tasks[step.taskId] : undefined;
        return [task?.id ?? null, task?.state ?? null, task ? taskState(task).word : null, task?.number ?? null];
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
  if (!plan) return null;

  const proposed = plan.state.type === "proposed";

  return (
    <AgentPlan
      data-card="plan"
      id={`plan-${plan.id}`}
      className={className}
      tabIndex={-1}
      title={plan.title}
      badges={
        <>
          {plan.risky && <Badge variant="warning">Risky</Badge>}
          <PlanStateBadge state={plan.state} />
        </>
      }
      steps={plan.steps.map((step, index) => {
        const taskId = steps[index * 4] as string | null | undefined;
        const state = steps[index * 4 + 1] as TaskState | null | undefined;
        const word = steps[index * 4 + 2] as string | null | undefined;
        const number = steps[index * 4 + 3] as number | null | undefined;
        const status = stepStatus(state ?? undefined);
        return {
          key: `${index}`,
          title: step.title,
          detail: step.detail,
          status,
          // The same words as the worker's row in the thread.
          statusLabel: word ?? "Planned",
          // Only the step at work shows its description; the rest open on demand.
          folded: status !== "active",
          // The worker carrying it out, named short: the step's title already says what it does.
          aside: taskId ? <WorkerChip taskId={taskId} label={`task-${number}`} /> : undefined,
        };
      })}
      footer={
        <>
          <PlanReview plan={plan} />
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
        </>
      }
    />
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
