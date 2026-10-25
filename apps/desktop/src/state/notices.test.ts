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
    previews: [],
    mcpServers: [],
    requests: [],
    compactions: [],
    peerMessages: [],
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

test("a saved notice shows its emails masked", () => {
  const board = emptyBoard("conversation");
  const saved: Notice = {
    level: "warning",
    atMs: 4,
    text: "Signing in as stephen@example.com failed; work@corp.io stays signed in.",
  };
  const masked = "Signing in as ste•••••@•••••.com failed; wor•••••@•••••.io stays signed in.";
  const view = {
    ...board,
    conversation: { id: board.conversationId },
    tasks: [],
    approvals: [],
    questions: [],
    previews: [],
    mcpServers: [],
    requests: [],
    compactions: [],
    peerMessages: [],
    notices: [saved, environment],
  } as unknown as ConversationView;
  assert.deepEqual(
    boardFromView(view, null, []).notices.map((notice) => notice.text),
    [masked, environment.text],
  );
  assert.equal(applyToBoard(board, event(saved)).notices[0]?.text, masked);
});

test("saved usage-limit and switch notices are hidden: the thread says them as dividers", () => {
  const board = emptyBoard("conversation");
  const saved: Notice[] = [
    "You've hit your session limit · resets 7:10pm",
    "Claude Code hit its usage limit; continuing on Codex (GPT-5.5) until 19:10, then back on Claude Code.",
    "Claude Code hit its usage limit on ste•••••@•••••.com; continuing on wor•••••@•••••.io.",
    "Claude Code is available again; continuing with it.",
    "Claude Code hit its usage limit on this computer's login; task 3 continues on wor•••••@•••••.io.",
    "Every model you allow is at its limit. Your message waits and goes on its own once a model you allow can take it.",
  ].map((text, index) => ({ level: "info", atMs: 10 + index, text }));
  const view = {
    ...board,
    conversation: { id: board.conversationId },
    tasks: [],
    approvals: [],
    questions: [],
    previews: [],
    mcpServers: [],
    requests: [],
    compactions: [],
    peerMessages: [],
    notices: [...saved, decode],
  } as unknown as ConversationView;
  assert.deepEqual(boardFromView(view, null, []).notices, [decode]);
  for (const notice of saved) assert.equal(applyToBoard(board, event(notice)), board);
});

test("a dismissed notice leaves the board for good; others stay (D51)", () => {
  let board = applyToBoard(applyToBoard(emptyBoard("conversation"), event(decode)), event(environment));
  const dismissed = { seq: 9, stream: "conversation", streamSeq: 9, atMs: 9, event: { type: "noticeDismissed" as const, conversationId: "conversation", atMs: decode.atMs } };
  board = applyToBoard(board, dismissed);
  assert.deepEqual(board.notices, [environment]);
  assert.equal(applyToBoard(board, dismissed), board);
});
