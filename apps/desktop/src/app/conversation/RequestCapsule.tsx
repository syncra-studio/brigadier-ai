import type { FC } from "react";
import { useShallow } from "zustand/react/shallow";

import { Capsule } from "@/app/conversation/Capsule";
import { CapsulePill } from "@/app/conversation/CapsulePill";
import { DiffPill } from "@/app/conversation/DiffPill";
import { mergeStats } from "@/app/conversation/mergeStats";
import {
  activePlanRequest,
  currentRequestPlan,
  planProgress,
  planStepStatus,
  stepStates,
} from "@/app/conversation/planProgress";
import { StepList } from "@/app/conversation/StepList";
import { Donut } from "@/components/conversation";
import { Clock, StopCircle, X } from "@/components/icons";
import type { DiffStat } from "@/ipc/generated";
import { openFileTab } from "@/state/sessionTabs";
import { useBoard } from "@/state/board";

/**
 * What a request changed so far, with the messages steered into it: what each recorded of its
 * own changes (`changes`: the thread's own edits and commands and its workers' landings, never
 * the user's), else, for a request from before that was kept, the stats of its workers' landed
 * commits. It keeps counting after the orchestrator's reply ended (the turn's diff reads it too),
 * so it is tied to the request, not to the turn. Nothing changed yet: `null`, never "0 files".
 */
export function useRequestDiff(requestId: string | null): DiffStat | null {
  const stats = useBoard(
    useShallow((s) => {
      if (requestId === null || !s.board) return [];
      const { requests, tasks } = s.board;
      const ids = Object.values(requests)
        .filter((request) => request.id === requestId || request.steeredInto === requestId)
        .map((request) => request.id);
      return ids.flatMap((id) => {
        const recorded = requests[id]?.changes;
        if (recorded) return [recorded.stat];
        return Object.values(tasks)
          .filter((task) => task.requestId === id && task.landed !== null)
          .toSorted((a, b) => a.number - b.number)
          .flatMap((task) => (task.candidate ? [task.candidate.diffStat] : []));
      });
    }),
  );
  const merged = mergeStats(stats);
  return merged && merged.files.length > 0 ? merged : null;
}

/** The newest revision belongs to this request, regardless of its lifecycle. */
function useRunningPlan(requestId: string | null) {
  const plan = useBoard((s) => currentRequestPlan(s.board?.plans ?? {}, requestId));
  const states = useBoard(
    useShallow((s) =>
      (plan?.steps ?? []).map((step) => step.taskId ? s.board?.tasks[step.taskId]?.state : undefined),
    ),
  );
  const request = useBoard((s) => (requestId ? s.board?.requests[requestId]?.state.type : undefined));
  return plan ? { plan, states: stepStates(plan, states, request) } : null;
}

/** "Phase N / M": the phase at work, else the first still to do, else the last. */
function phaseCount(statuses: readonly ReturnType<typeof planStepStatus>[]): string {
  const at = statuses.indexOf("active");
  const next = statuses.findIndex((status) => status !== "done");
  const phase = at >= 0 ? at : next >= 0 ? next : statuses.length - 1;
  return `Phase ${phase + 1} / ${statuses.length}`;
}

/** A request's capsule outside a run: its plan's step and what its workers landed. */
export const RequestCapsule: FC<{ conversationId: string }> = ({ conversationId }) => {
  const requestId = useBoard((s) =>
    s.board?.conversationId === conversationId ? activePlanRequest(s.board.requests)?.id ?? null : null,
  );
  const diff = useRequestDiff(requestId);
  const running = useRunningPlan(requestId);
  if (!requestId || (!diff && !running)) return null;
  const progress = running ? planProgress(running.plan, running.states) : null;
  const statuses = running ? running.plan.steps.map((_, index) => planStepStatus(running.states[index])) : [];
  // A one-step plan counts nothing ("Phase 1 / 1"): its pill only says what needs saying.
  const pill = !!progress && (statuses.length > 1 || progress.status === "review" || progress.status === "failed" || progress.status === "stopped");
  if (!diff && !pill) return null;
  return (
    <Capsule>
      {running && progress && pill && (
        <CapsulePill
          label={`Plan steps: ${running.plan.title}`}
          tip={
            <StepList
              steps={running.plan.steps.map((step, index) => ({
                title: step.title,
                status: planStepStatus(running.states[index]),
              }))}
            />
          }
        >
          {progress.status === "review" ? (
            <>
              <Clock aria-hidden className="size-icon-xs shrink-0" />
              <span className="text-foreground min-w-0 truncate">{progress.label}</span>
            </>
          ) : progress.status === "failed" ? (
            <>
              <X aria-hidden className="text-destructive size-icon-xs shrink-0" />
              <span className="text-foreground min-w-0 truncate">{progress.label}</span>
            </>
          ) : progress.status === "stopped" ? (
            <>
              <StopCircle aria-hidden className="text-muted-foreground size-icon-xs shrink-0" />
              <span className="text-muted-foreground min-w-0 truncate">{progress.label}</span>
            </>
          ) : (
            <>
              <Donut done={statuses.filter((status) => status === "done").length} total={statuses.length} />
              <span className="text-foreground whitespace-nowrap tabular-nums">{phaseCount(statuses)}</span>
            </>
          )}
        </CapsulePill>
      )}
      {running && pill && diff && <span aria-hidden>·</span>}
      {diff && (
        <DiffPill
          diff={diff}
          {...(diff.files[0] && { onOpen: () => openFileTab(conversationId, diff.files[0]!.path) })}
        />
      )}
    </Capsule>
  );
};
