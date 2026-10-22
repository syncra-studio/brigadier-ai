import { type FC, useContext } from "react";

import { isRunRequest } from "@/app/conversation/blocks";
import { ComposerTargetContext } from "@/app/conversation/composerTarget";
import { activeRequest, RequestCapsule } from "@/app/conversation/RequestCapsule";
import { type Board, useBoard } from "@/state/board";

/** Something waits for the user's decision: the capsule makes room for its card. */
function anyPending(board: Board | null, conversationId: string): boolean {
  if (!board || board.conversationId !== conversationId) return false;
  return (
    Object.values(board.approvals).some((approval) => approval.state.type === "pending") ||
    Object.values(board.questions).some((question) => question.answeredAtMs === null)
  );
}

/** The active request's landed diff. */
export const ComposerCapsule: FC = () => {
  const conversationId = useContext(ComposerTargetContext)?.conversation?.id ?? "";
  const pending = useBoard((s) => anyPending(s.board, conversationId));
  const request = useBoard((s) =>
    s.board?.conversationId === conversationId ? activeRequest(s.board.requests) : null,
  );
  if (pending || !request || isRunRequest(request.id) || request.state.type !== "working") return null;
  return <RequestCapsule conversationId={conversationId} />;
};
