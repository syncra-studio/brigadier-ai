import type { Conversation } from "@/ipc/generated";
import type { Board } from "@/state/board";

export type PendingAction = {
  type: "approval" | "question";
  id: string;
};

type PendingBoard = Pick<Board, "conversationId" | "approvals" | "questions">;

/** Oldest pending decisions first. */
export function pendingActionKeys(
  conversation: Pick<Conversation, "id" | "setup"> | null,
  board: PendingBoard | null,
): string[] {
  if (!board || board.conversationId !== conversation?.id) return [];
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
  return waiting.toSorted((a, b) => a.position - b.position).map((entry) => entry.key);
}
