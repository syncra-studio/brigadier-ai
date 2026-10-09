import { actionWords, batchesOf, outcomeWords, summaryWords } from "@/app/conversation/computerSteps";
import type { ComputerAction, EventEnvelope } from "@/ipc/generated";
import { applyToBoard, emptyBoard } from "@/state/board";

/** What the timeline shows after each event: its line, its batches, and the newest batch's steps. */
export type ComputerFrame = {
  summary: string;
  batches: number;
  shown: { words: string; outcome: string; failed: boolean }[];
  image: string | null;
};

/** Feeds a worker's recorded actions through the board, as live events, after its timeline was opened. */
export function replayComputer(conversationId: string, taskId: string, actions: readonly ComputerAction[]): ComputerFrame[] {
  let board = emptyBoard(conversationId);
  board = { ...board, computer: { [taskId]: { actions: [], earlier: null, loading: false } } };
  const frames: ComputerFrame[] = [];
  actions.forEach((action, i) => {
    const envelope: EventEnvelope = { seq: i + 1, streamSeq: i + 1, stream: `conversation:${conversationId}`, atMs: action.atMs,
      event: { type: "computerActed", conversationId, taskId, action } };
    board = applyToBoard(board, envelope);
    const log = board.computer[taskId]?.actions ?? [];
    const batches = batchesOf(log);
    const last = batches.at(-1);
    frames.push({
      summary: summaryWords(log, false),
      batches: batches.length,
      shown: (last?.actions ?? []).map((step) => {
        const outcome = outcomeWords(step);
        return { words: actionWords(step), outcome: outcome.text, failed: outcome.failed };
      }),
      image: last?.image ?? null,
    });
  });
  return frames;
}
