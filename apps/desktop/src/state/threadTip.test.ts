import assert from "node:assert/strict";
import { test } from "node:test";

import type { EventEnvelope } from "@/ipc/generated";
import { applyToBoard, emptyBoard } from "@/state/board";

const seen = (tip: string, seq: number): EventEnvelope =>
  ({
    stream: "conversation:c",
    streamSeq: seq,
    atMs: seq,
    event: { type: "threadCommitsSeen", conversationId: "c", branch: "brigadier/c/session", tip, authoredFrom: "a0" },
  }) as unknown as EventEnvelope;

// The thread's own commit (a fix before its request ends) moves the branch: the board keeps the
// tip, which the context card's Changes count reads again on (#412).
test("each commit of the thread's moves the board's tip of its branch", () => {
  let board = emptyBoard("c");
  assert.equal(board.threadTip, null);
  board = applyToBoard(board, seen("f9ac5d2", 1));
  assert.equal(board.threadTip, "f9ac5d2");
  board = applyToBoard(board, seen("e4206df", 2));
  assert.equal(board.threadTip, "e4206df");
});
