import assert from "node:assert/strict";
import { test } from "node:test";

import { blockSequence, buildBlocks } from "@/app/conversation/blocks";
import type { ConversationView, DomainEvent, EventEnvelope, Message } from "@/ipc/generated";
import { isPlumbing, stepLabel, toolHasOwnResult, toolStepWords } from "@/app/conversation/activity/words";
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
    state: { type: "working" }, startedAtMs: 0, endedAtMs: null, steeredInto: null, steeredAfter: null, undo: null, worked: [], quotaWait: false } }),
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

const tool = (itemId: string, name: string, status: "inProgress" | "completed" | "failed", atMs: number, detail: string | null = null): DomainEvent => ({
  type: "orchestratorStepped", step: { requestId: "request", atMs, position: 0,
    kind: { type: "tool", itemId, name, detail, status, throughPosition: 0 } },
});

test("tool-only Claude turns stay visible through the recorded initial gap", () => {
  // Phase E offsets from the user's first message. No reasoning is needed for these rows.
  const calls = [
    envelope(3, 3719, tool("wrong-name", "query_brain", "inProgress", 3719)),
    envelope(4, 3721, tool("wrong-name", "query_brain", "failed", 3721)),
    envelope(5, 5093, tool("brain", "query_brain", "inProgress", 5093)),
    envelope(6, 5113, tool("brain", "query_brain", "completed", 5113, "composer attachments")),
    envelope(7, 31396, tool("delegate", "delegate_task", "inProgress", 31396)),
  ];
  const frames = replayThinking(conversationId, [...events.slice(0, 2), ...calls], 0, 32000);
  assert.equal(frames.filter((frame) => frame.onlyThinking).length, 4);
  assert.ok(frames.filter((frame) => frame.atMs >= 4000).every((frame) => !frame.onlyThinking));
  assert.ok(frames.every((frame) => frame.liveText === null));
});

test("tool updates and concurrent snapshot replay keep one row at its first position", () => {
  const calls = [envelope(3, 3000, tool("brain", "query_brain", "inProgress", 3000)),
    envelope(4, 4000, tool("brain", "query_brain", "completed", 4000, "composer"))];
  const board = [...events.slice(0, 2), ...calls].reduce(applyToBoard, emptyBoard(conversationId));
  assert.equal(board.orchestratorSteps.length, 1);
  const step = board.orchestratorSteps[0]!;
  assert.equal(step.position, 3);
  assert.equal(step.atMs, 3000);
  assert.equal(step.kind.type, "tool");
  assert.equal(applyToBoard(board, calls[0]!), board);
  assert.equal(applyToBoard(board, calls[1]!), board);
  const view = { conversation: { id: conversationId }, tasks: [], approvals: [], questions: [], plans: [],
    overnight: [], requests: Object.values(board.requests), workerSteps: [], orchestratorSteps: board.orchestratorSteps,
    machineSteps: [], decisions: [], waiting: [], compactions: [], ratings: {}, queue: board.queue,
    run: "running", runRequest: "request", head: message.id, context: null, streaming: null,
    thinking: [], notices: [], memories: [] } as unknown as ConversationView;
  assert.deepEqual(boardFromView(view, null, calls).orchestratorSteps, board.orchestratorSteps);
});

test("delegation calls stay out of the parent thread while their stored events remain available", () => {
  const call = envelope(3, 3000, tool("delegate", "delegate_task", "inProgress", 3000));
  let board = [...events.slice(0, 2), call].reduce(applyToBoard, emptyBoard(conversationId));
  let block = buildBlocks([message], {}, false, board, [])[0]!;
  assert.equal(block.orchestratorSteps.length, 0);
  board = applyToBoard(board, envelope(4, 4000, tool("delegate", "delegate_task", "completed", 4000)));
  block = buildBlocks([message], {}, false, board, [])[0]!;
  // A successful call is kept until its authored result arrives, including on replay.
  assert.equal(block.orchestratorSteps.length, 0);
  board = applyToBoard(board, envelope(5, 4100, { type: "orchestratorStepped", step: {
    requestId: "request", atMs: 4100, position: 0, kind: { type: "created", taskId: "worker" },
  } }));
  block = buildBlocks([message], {}, false, board, [])[0]!;
  assert.equal(block.orchestratorSteps.filter((step) => step.kind.type === "tool").length, 0);
  board = applyToBoard(board, envelope(6, 5000, tool("delegate", "delegate_task", "failed", 5000)));
  block = buildBlocks([message], {}, false, board, [])[0]!;
  assert.equal(block.orchestratorSteps.filter((step) => step.kind.type === "tool").length, 0);
  const kind = board.orchestratorSteps[0]!.kind;
  assert.ok(kind.type === "tool");
  // A call to a worker is plumbing: its worker's sentence says what came of it.
  assert.ok(isPlumbing(kind.name));
  assert.equal(toolHasOwnResult(kind), false);
  assert.equal(stepLabel(toolStepWords({ ...kind, name: "Read", detail: "README.md" }), "completed"), "Read README.md");
});
