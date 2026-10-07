import assert from "node:assert/strict";
import { test } from "node:test";

import type { ConversationView, Notice } from "@/ipc/generated";
import { applyToBoard, boardFromView, emptyBoard } from "./board";

const drift: Notice = {
  level: "warning",
  atMs: 1,
  text: "Codex 0.159.2 is running; Brigadier's bindings were generated from 0.158.0. Unknown fields are ignored; regenerate them with gen-codex.",
};
const decode: Notice = {
  level: "warning",
  atMs: 2,
  text: "Could not decode a Codex event: missing field type",
};
const environment: Notice = {
  level: "info",
  atMs: 3,
  text: "Using a different shell",
};

const event = (notice: Notice) => ({
  seq: notice.atMs,
  stream: "conversation",
  streamSeq: notice.atMs,
  atMs: notice.atMs,
  event: {
    type: "conversationNotice" as const,
    conversationId: "conversation",
    notice,
  },
});

test("stored version drift disappears when a conversation is reopened; actual warnings survive", () => {
  const board = emptyBoard("conversation");
  const view = {
    ...board,
    conversation: { id: board.conversationId },
    tasks: [],
    approvals: [],
    questions: [],
    plans: [],
    reviews: [],
    overnight: [],
    requests: [],
    waiting: [],
    compactions: [],
    notices: [drift, decode, environment],
  } as unknown as ConversationView;
  assert.deepEqual(boardFromView(view, null, []).notices, [
    decode,
    environment,
  ]);
});

test("older providers' live drift notices are hidden, but decode failures still reach the thread", () => {
  const board = emptyBoard("conversation");
  assert.equal(applyToBoard(board, event(drift)), board);
  assert.deepEqual(applyToBoard(board, event(decode)).notices, [decode]);
});
