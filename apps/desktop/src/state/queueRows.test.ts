import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { Conversation, EventEnvelope, Message, QueuedMessage, SendOutcome } from "@/ipc/generated";
import { send } from "@/state/actions";
import { applyActivityNews } from "@/state/activity";
import { applyBoardEvents, emptyBoard, useBoard } from "@/state/board";
import { queueRows } from "@/state/queueRows";
import { applyEvents, emptyThread, useApp } from "@/state/store";

const id = "queue-rows";
const text = "Also add CSV.";
const item: QueuedMessage = { id: "queued", text, attachments: [], mentions: [], queuedAtMs: 20, editedAtMs: null, deciding: true };
const other: QueuedMessage = { ...item, id: "other", text: "And the docs.", deciding: false };
const message: Message = { id: "sent", conversationId: id, seq: 3, role: "user", text, attachments: [], mentions: [], createdAtMs: 30, blob: null, model: null, requestId: "sent", parentId: null };
let reply: (outcome: SendOutcome) => void;
let refuse: (error: unknown) => void;
const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window");

beforeEach(() => {
  Object.defineProperty(globalThis, "window", { configurable: true, value: {} });
  useApp.setState({
    selection: { type: "conversation", id },
    conversations: { [id]: { id, kind: "session" } as Conversation },
    pending: [],
    threads: { [id]: emptyThread },
  });
  useBoard.setState({ board: emptyBoard(id) });
  // The session works: its CLI's turn is open.
  applyActivityNews({ conversationId: id, entries: [], opened: true });
  mockIPC(() => new Promise((resolve, reject) => {
    reply = (outcome) => resolve({ method: "sendMessage", outcome });
    refuse = reject;
  }));
});
afterEach(() => {
  clearMocks();
  if (originalWindow) Object.defineProperty(globalThis, "window", originalWindow);
  else Reflect.deleteProperty(globalThis, "window");
});

function queue(items: QueuedMessage[], seq: number) {
  const envelope = { event: { type: "queueChanged", conversationId: id, queue: { items, paused: false } }, streamSeq: seq, atMs: seq, seq, stream: `conversation:${id}` } as EventEnvelope;
  applyEvents([envelope]);
  applyBoardEvents([envelope]);
}
/** The queue rail's rows: (text, shown before the daemon has it, deciding). */
function rail() {
  const { board } = useBoard.getState();
  return queueRows(board?.queue.items ?? [], useApp.getState().pending.filter((entry) => entry.conversationId === id))
    .map((row) => [row.text, !!row.local, row.deciding]);
}
/** What the thread shows: its messages and the sends it shows before the daemon has them. */
function inThread() {
  return (useApp.getState().threads[id]?.items.length ?? 0) + useApp.getState().pending.filter((entry) => !entry.queued).length;
}

// #491: the rail showed a message sent while the session worked only once the daemon answered,
// seconds later while its connection was busy, and nothing showed it meanwhile.
test("a message sent while a session works shows in the queue rail at once, deciding", async () => {
  const sent = send({ text, attachments: [], mentions: [] });
  assert.deepEqual(rail(), [[text, true, true]]);
  assert.equal(inThread(), 0);
  reply({ type: "queued", item });
  await sent;
  assert.deepEqual(rail(), [[text, false, true]]);
  assert.equal(inThread(), 0);
});

test("a message put back in its slot shows there at once", async () => {
  queue([other, { ...other, id: "third", text: "Then the tests." }], 1);
  const sent = send({ text, attachments: [], mentions: [] }, undefined, { lane: "queue", queueIndex: 1 });
  assert.deepEqual(rail().map(([words]) => words), ["And the docs.", text, "Then the tests."]);
  reply({ type: "queued", item: { ...item, deciding: false } });
  await sent;
  assert.deepEqual(rail().map(([words, local]) => [words, local]), [["And the docs.", false], [text, false], ["Then the tests.", false]]);
});

test("its queue event takes its row's place before the answer, and the answer adds nothing", async () => {
  const sent = send({ text, attachments: [], mentions: [] });
  queue([item], 1);
  assert.deepEqual(rail(), [[text, false, true]]);
  reply({ type: "queued", item });
  await sent;
  assert.deepEqual(rail(), [[text, false, true]]);
});

test("a queue change elsewhere before the answer keeps its row, and the answer puts its item there", async () => {
  const sent = send({ text, attachments: [], mentions: [] });
  queue([other], 1);
  assert.deepEqual(rail(), [["And the docs.", false, false], [text, true, true]]);
  reply({ type: "queued", item });
  await sent;
  assert.deepEqual(rail(), [["And the docs.", false, false], [text, false, true]]);
});

test("one the daemon sent on leaves the rail for the thread", async () => {
  const sent = send({ text, attachments: [], mentions: [] });
  reply({ type: "sent", message });
  await sent;
  assert.deepEqual(rail(), []);
  assert.equal(inThread(), 1);
});

test("a refused send's row leaves the rail", async () => {
  const sent = send({ text, attachments: [], mentions: [] });
  assert.equal(rail().length, 1);
  refuse(new Error("at most 50 messages can wait in the queue"));
  await assert.rejects(sent);
  assert.deepEqual(rail(), []);
});

// #494: a message sent right after another showed in the thread first, then moved to the queue
// rail: the board didn't show the first one's turn yet, though the daemon already worked on it.
test("a send while the last one's turn hasn't reached the board yet waits in the rail", async () => {
  useBoard.setState({ board: emptyBoard(id) });
  const first = { ...message, id: "first-sent", requestId: "first-sent", createdAtMs: Date.now() };
  useApp.setState({ threads: { [id]: { ...emptyThread, items: [first] } } });
  const sent = send({ text, attachments: [], mentions: [] });
  assert.deepEqual(rail(), [[text, true, true]]);
  assert.equal(inThread(), 1);
  reply({ type: "queued", item });
  await sent;
});

test("a send while the last one is still under way waits in the rail", async () => {
  useBoard.setState({ board: emptyBoard(id) });
  applyActivityNews({ conversationId: id, entries: [], ended: "done" });
  const firstSend = send({ text: "Build the export", attachments: [], mentions: [] });
  assert.equal(inThread(), 1);
  const replyFirst = reply;
  const sent = send({ text, attachments: [], mentions: [] });
  assert.deepEqual(rail(), [[text, true, true]]);
  replyFirst({ type: "sent", message: { ...message, id: "first-sent", requestId: "first-sent", text: "Build the export" } });
  reply({ type: "queued", item });
  await Promise.all([firstSend, sent]);
});

test("an idle session's message still goes straight to the thread", async () => {
  useBoard.setState({ board: emptyBoard(id) });
  applyActivityNews({ conversationId: id, entries: [], ended: "done" });
  const old = { ...message, id: "old", requestId: "old", createdAtMs: Date.now() - 60_000 };
  useApp.setState({ threads: { [id]: { ...emptyThread, items: [old] } } });
  const sent = send({ text, attachments: [], mentions: [] });
  assert.deepEqual(rail(), []);
  assert.equal(inThread(), 2);
  reply({ type: "sent", message });
  await sent;
});
