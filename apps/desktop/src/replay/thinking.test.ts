import assert from "node:assert/strict";
import { test } from "node:test";

import { blockSequence, buildBlocks } from "@/app/conversation/blocks";
import type { ConversationView, DomainEvent, EventEnvelope, Message } from "@/ipc/generated";
import { replayThinking } from "@/replay/thinking";
import { applyToBoard, boardFromView, emptyBoard } from "@/state/board";

const conversationId = "thinking-replay";
const message = { id: "request", requestId: "request", role: "user", seq: 1, conversationId,
  text: "Check the settings", blob: null, createdAtMs: 0, attachments: [], mentions: [], model: null, parentId: null } as Message;
function envelope(seq: number, atMs: number, event: DomainEvent): EventEnvelope {
  return { seq, streamSeq: seq, stream: `conversation:${conversationId}`, atMs, event };
}
const reason = (text: string, atMs: number, itemId = "first", complete = false): DomainEvent => ({
  type: "thinkingDelta", conversationId, itemId, requestId: "request", text, atMs, complete,
});
const events: EventEnvelope[] = [
  envelope(1, 0, { type: "messageAppended", message }),
  envelope(2, 0, { type: "requestUpdated", request: { id: "request", conversationId, preview: message.text,
    state: { type: "working" }, startedAtMs: 0, endedAtMs: null, steeredInto: null, steeredAfter: null, undo: null } }),
  envelope(3, 2000, reason("I will check the settings", 2000)),
  envelope(4, 6000, reason(" and find where the choice is stored.", 6000)),
  envelope(5, 12000, { type: "orchestratorStepped", step: { requestId: "request", atMs: 12000, position: 5,
    kind: { type: "readArtifact", name: "settings.ts" } } }),
  envelope(6, 14000, reason("The existing store can keep the theme.", 14000, "second")),
];

test("reasoning is visible within two seconds and interleaves with actions in the real row derivation", () => {
  const frames = replayThinking(conversationId, events, 0, 32000);
  assert.equal(frames.filter((frame) => frame.onlyThinking).length, 2);
  assert.ok(frames.filter((frame) => frame.atMs >= 2000).every((frame) => !frame.onlyThinking));
  assert.equal(frames[6]!.liveText, "I will check the settings and find where the choice is stored.");
  assert.deepEqual(frames[14]!.kinds, ["thinking", "orchestrator", "thinking"]);
  assert.equal(frames[14]!.liveText, "The existing store can keep the theme.");
});

test("a snapshot read replays reasoning deltas once and completion replaces streamed text", () => {
  let board = events.slice(0, 4).reduce(applyToBoard, emptyBoard(conversationId));
  const view = { conversation: { id: conversationId }, tasks: [], approvals: [], questions: [], plans: [],
    overnight: [], requests: Object.values(board.requests), workerSteps: [], orchestratorSteps: [], machineSteps: [],
    decisions: [], waiting: [], compactions: [], ratings: {}, queue: board.queue, run: "running", runRequest: "request",
    head: message.id, context: null, streaming: null, thinking: board.thinking, notices: [], memories: [] } as unknown as ConversationView;
  board = boardFromView(view, null, events.slice(2, 4));
  assert.equal(board.thinking[0]!.text, "I will check the settings and find where the choice is stored.");
  board = applyToBoard(board, envelope(7, 17000, reason("The final summary.", 17000, "first", true)));
  assert.equal(board.thinking[0]!.text, "The final summary.");
  assert.equal(board.thinking[0]!.position, 3);
  const block = buildBlocks([message], {}, false, board, [])[0]!;
  const rows = blockSequence({ ...block, state: "stopped", steers: [] });
  assert.ok(rows.every((row) => row.kind !== "thinking" || !row.live));
  // Views created before reasoning was added still load.
  assert.deepEqual(boardFromView({ ...view, thinking: undefined } as unknown as ConversationView, null, []).thinking, []);
});
