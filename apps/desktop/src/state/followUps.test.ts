import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { EventEnvelope, Message, Mention, QueuedMessage, SendOutcome } from "@/ipc/generated";
import { send } from "@/state/actions";
import { applyBoardEvents, emptyBoard, useBoard } from "@/state/board";
import { applyEvents, emptyThread, useApp } from "@/state/store";

const id = "follow-ups";
const item: QueuedMessage = { id: "queued", text: "Also fix this", attachments: [], mentions: [], queuedAtMs: 20, editedAtMs: null, deciding: true };
const message: Message = { id: "joined", conversationId: id, seq: 3, role: "user", text: item.text, attachments: [], mentions: [], createdAtMs: 30, blob: null, model: null, requestId: "joined", parentId: null };
let reply: (outcome: SendOutcome) => void;
const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window");

beforeEach(() => {
  Object.defineProperty(globalThis, "window", { configurable: true, value: {} });
  useApp.setState({ selection: { type: "conversation", id }, pending: [], threads: { [id]: emptyThread } });
  useBoard.setState({ board: emptyBoard(id) });
  mockIPC(() => new Promise((resolve) => { reply = (outcome) => resolve({ method: "sendMessage", outcome }); }));
});
afterEach(() => {
  clearMocks();
  if (originalWindow) Object.defineProperty(globalThis, "window", originalWindow);
  else Reflect.deleteProperty(globalThis, "window");
});

function receive(event: EventEnvelope["event"], streamSeq: number) {
  const envelope = { event, streamSeq, atMs: streamSeq, seq: streamSeq, stream: `conversation:${id}` } as EventEnvelope;
  applyEvents([envelope]);
  applyBoardEvents([envelope]);
}
function queue(items: QueuedMessage[], seq = 1) {
  receive({ type: "queueChanged", conversationId: id, queue: { items, paused: false } }, seq);
}
function start() { return send({ text: item.text, attachments: item.attachments, mentions: [] }); }
function chatCount() { return useApp.getState().pending.length + (useApp.getState().threads[id]?.items.length ?? 0); }

test("a deciding queue event removes the stale-board echo before a delayed send response", async () => {
  const sent = start();
  assert.equal(chatCount(), 1, "idle sends still echo immediately");
  queue([item]);
  assert.equal(chatCount(), 0);
  assert.equal(useBoard.getState().board?.queue.items.length, 1);
  reply({ type: "queued", item });
  await sent;
  assert.equal(chatCount(), 0);
});

test("a queued response clears the echo before publishing the rail item", async () => {
  const sent = start();
  const duplicates: number[] = [];
  const stop = useBoard.subscribe(({ board }) => { if (board?.queue.items.length) duplicates.push(chatCount()); });
  reply({ type: "queued", item });
  await sent;
  stop();
  assert.deepEqual(duplicates, [0]);
});

test("queued and removed follow-ups leave no ghost; a late response cannot restore them", async () => {
  const sent = start();
  queue([item]);
  queue([{ ...item, deciding: false }], 2);
  assert.equal(chatCount(), 0);
  queue([], 3);
  reply({ type: "queued", item });
  await sent;
  assert.equal(chatCount(), 0);
  assert.equal(useBoard.getState().board?.queue.items.length, 0);
});

test("joining admits the user message at its join position and a late response cannot duplicate it", async () => {
  const sent = start();
  queue([item]);
  queue([], 2);
  receive({ type: "messageAppended", message }, 3);
  reply({ type: "queued", item });
  await sent;
  assert.equal(chatCount(), 1);
  assert.equal(useBoard.getState().board?.queue.items.length, 0);
  assert.equal(useApp.getState().threads[id]?.items[0]?.seq, 3);
});

test("an idle send goes straight to the conversation and its event replaces its echo", async () => {
  const sent = start();
  assert.equal(chatCount(), 1);
  receive({ type: "messageAppended", message }, 1);
  reply({ type: "sent", message });
  await sent;
  assert.equal(chatCount(), 1);
  assert.equal(useApp.getState().pending.length, 0);
  assert.equal(useBoard.getState().board?.queue.items.length, 0);
});

test("replaying or settling a queue item does not consume a second identical send", () => {
  const pending = { localId: "first", conversationId: id, text: item.text, attachments: [], createdAtMs: 10 };
  useApp.setState({ pending: [pending, { ...pending, localId: "second" }] });
  queue([item]);
  assert.deepEqual(useApp.getState().pending.map((entry) => entry.localId), ["second"]);
  queue([{ ...item, deciding: false }], 2);
  assert.deepEqual(useApp.getState().pending.map((entry) => entry.localId), ["second"]);
  queue([item, { ...item, id: "second-item" }], 3);
  assert.equal(useApp.getState().pending.length, 0);
});

test("an older identical queued item cannot consume a new send", async () => {
  queue([item]);
  const sent = start();
  queue([{ ...item, deciding: false }], 2);
  assert.equal(useApp.getState().pending.length, 1);
  reply({ type: "sent", message });
  await sent;
  assert.equal(chatCount(), 1);
});

test("queue reconciliation distinguishes attachments and conversations", () => {
  const attachment = { id: "image", name: "image.png", mime: "image/png", bytes: 1, pasted: false, inline: null };
  const pending = { localId: "plain", conversationId: id, text: item.text, attachments: [], createdAtMs: 10 };
  useApp.setState({ pending: [pending, { ...pending, localId: "elsewhere", conversationId: "other", attachments: [attachment] }, { ...pending, localId: "image", attachments: [attachment] }] });
  queue([{ ...item, attachments: [attachment] }]);
  assert.deepEqual(useApp.getState().pending.map((entry) => entry.localId), ["plain", "elsewhere"]);
});

test("a queue event with no matching pending entries keeps pending the same", () => {
  const pending = useApp.getState().pending;
  queue([item]);
  assert.equal(useApp.getState().pending, pending);
});

test("a queued response identifies its item before its event can consume another echo", async () => {
  const sent = start();
  useApp.setState((state) => ({ pending: [...state.pending, { localId: "second", conversationId: id, text: item.text, attachments: [], createdAtMs: 10 }] }));
  reply({ type: "queued", item });
  await sent;
  queue([item]);
  assert.deepEqual(useApp.getState().pending.map((entry) => entry.localId), ["second"]);
});

test("a queued item matches the send's mentions as well as its visible text", async () => {
  const mentions: [Mention, Mention, Mention] = [{ type: "file", path: "first.ts" }, { type: "task", id: "one" }, { type: "chat", id: "chat-one", title: "Same title" }];
  const sent = send({ text: item.text, attachments: [], mentions });
  assert.deepEqual(useApp.getState().pending[0]?.mentions, mentions);
  queue([{ ...item, mentions: [{ type: "file", path: "second.ts" }, mentions[1], mentions[2]] }]);
  assert.equal(useApp.getState().pending.length, 1);
  queue([{ ...item, id: "task-mention", mentions: [mentions[0], { type: "task", id: "two" }, mentions[2]] }], 2);
  assert.equal(useApp.getState().pending.length, 1);
  queue([{ ...item, id: "chat-mention", mentions: [mentions[0], mentions[1], { type: "chat", id: "chat-two", title: "Same title" }] }], 3);
  assert.equal(useApp.getState().pending.length, 1);
  const matching = { ...item, id: "matching-mentions", mentions };
  queue([matching], 4);
  assert.equal(useApp.getState().pending.length, 0);
  reply({ type: "queued", item: matching });
  await sent;
});

test("an idle admission preceding an identical follow-up stays in chat while that follow-up queues", async () => {
  const sent = start();
  useApp.setState((state) => ({ pending: [...state.pending, { localId: "second", conversationId: id, text: item.text, attachments: [], createdAtMs: 10 }] }));
  // The daemon records the first admission before that request can cause the next send to queue.
  receive({ type: "messageAppended", message }, 1);
  queue([item], 2);
  assert.equal(useApp.getState().threads[id]?.items.length, 1);
  assert.equal(useApp.getState().pending.length, 0);
  assert.equal(useBoard.getState().board?.queue.items.length, 1);
  reply({ type: "sent", message });
  await sent;
  assert.equal(chatCount(), 1);
});
