import { type FC, useContext, useMemo } from "react";

import { ComposerTargetContext } from "@/app/conversation/composerTarget";
import { type RunPill, runPill } from "@/app/conversation/phaseView";
import { activePlanRequest, capsuleMode, currentRequestPlan } from "@/app/conversation/planProgress";
import { RequestCapsule } from "@/app/conversation/RequestCapsule";
import { RunCapsule } from "@/app/conversation/RunCapsule";
import { type Board, useBoard } from "@/state/board";

/** Something waits for the user's decision: the capsule makes room for its card. */
function anyPending(board: Board | null, conversationId: string): boolean {
  if (!board || board.conversationId !== conversationId) return false;
  return (
    Object.values(board.approvals).some((approval) => approval.state.type === "pending") ||
    Object.values(board.questions).some((question) => question.answeredAtMs === null) ||
    Object.values(board.plans).some((plan) => plan.state.type === "proposed")
  );
}

/** Where the session's active run is, if one is at work. */
function useRunPill(conversationId: string): RunPill | null {
  const runs = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.overnight : null));
  const plans = useBoard((s) => s.board?.plans);
  return useMemo(() => (runs && plans ? runPill(runs, plans) : null), [runs, plans]);
}

/**
 * The active request's plan progress and landed diff. Overnight runs keep their phase capsule.
 */
export const ComposerCapsule: FC = () => {
  const conversationId = useContext(ComposerTargetContext)?.conversation?.id ?? "";
  const pending = useBoard((s) => anyPending(s.board, conversationId));
  const run = useRunPill(conversationId);
  const request = useBoard((s) =>
    s.board?.conversationId === conversationId ? activePlanRequest(s.board.requests) : null,
  );
  const plan = useBoard((s) => currentRequestPlan(s.board?.plans ?? {}, request?.id ?? null));
  const mode = capsuleMode(run !== null, pending, request, plan);
  if (mode === null) return null;
  return mode === "run" && run ? <RunCapsule conversationId={conversationId} run={run} /> : <RequestCapsule conversationId={conversationId} />;
};
