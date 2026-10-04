import type { Conversation } from "@/ipc/generated";
import type { Board } from "@/state/board";

export type PendingAction = {
  type: "approval" | "question" | "plan" | "overnight";
  id: string;
};

type PendingBoard = Pick<Board, "conversationId" | "approvals" | "questions" | "plans" | "overnight">;

/** Oldest pending decisions first, with overnight proposals after the other cards. */
export function pendingActionKeys(
  conversation: Pick<Conversation, "id" | "setup"> | null,
  board: PendingBoard | null,
): string[] {
  if (!board || board.conversationId !== conversation?.id) return [];
  // Plan mode hands the plan to the user whatever the permission level.
  const decidesPlans = conversation.setup?.type === "session" &&
    (conversation.setup.permission === "askForApproval" || conversation.setup.planMode);
  const waiting: { key: string; position: number }[] = [];
  for (const approval of Object.values(board.approvals)) {
    if (approval.state.type === "pending") {
      waiting.push({ key: `approval:${approval.id}`, position: approval.position });
    }
  }
  for (const question of Object.values(board.questions)) {
    if (question.answer === null && question.answeredAtMs === null) {
      waiting.push({ key: `question:${question.id}`, position: question.position });
    }
  }
  if (decidesPlans) {
    for (const plan of Object.values(board.plans)) {
      if (plan.state.type === "proposed") {
        waiting.push({ key: `plan:${plan.id}`, position: plan.position });
      }
    }
  }
  for (const run of Object.values(board.overnight)) {
    if (run.state === "proposed") {
      waiting.push({ key: `overnight:${run.id}`, position: Number.MAX_SAFE_INTEGER });
    }
  }
  return waiting.toSorted((a, b) => a.position - b.position).map((entry) => entry.key);
}
