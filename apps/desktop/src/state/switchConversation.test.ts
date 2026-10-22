import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";
import { mockIPC } from "@tauri-apps/api/mocks";

import type { RequestOf } from "@/ipc/client";
import type { Conversation, ConversationView, EventEnvelope, Message, RunState } from "@/ipc/generated";

const data = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => data.get(key) ?? null,
  setItem: (key: string, value: string) => data.set(key, value),
  removeItem: (key: string) => data.delete(key),
} });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage, setTimeout, clearTimeout } });
const { openSettings, select } = await import("./actions");
const { applyBoardEvents, forgetKeptBoards, useBoard } = await import("./board");
const { useApp } = await import("./store");

const conversation = (id: string) => ({ id, kind: "session", title: id, sideOf: null }) as unknown as Conversation;
const message = (id: string, seq: number): Message => ({
  id: `${id}-m${seq}`, conversationId: id, seq, role: seq % 2 ? "user" : "assistant", text: `message ${seq}`, attachments: [], mentions: [],
  createdAtMs: seq, blob: null, model: null, requestId: null, parentId: null,
});

/** Each conversation's run state as the daemon has it. */
let runs: Record<string, RunState>;
/** View reads the daemon has not answered yet: a slow daemon (#484). */
let held: (() => void)[];
let slow: boolean;

function view(id: string): ConversationView {
  return {
    conversation: conversation(id), context: null, messages: { messages: [message(id, 1), message(id, 2)], hasMore: false },
    tasks: [], approvals: [], questions: [], requests: [], queue: { items: [], paused: false }, run: runs[id] ?? "idle",
    runRequest: null, head: null, workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [], compactions: [],
    decisions: [], ratings: {}, streaming: null, notices: [], memories: [],
  } as unknown as ConversationView;
}

beforeEach(() => {
  runs = { a: "running", b: "idle", c: "running" };
  held = [];
  slow = false;
  forgetKeptBoards?.();
  useBoard.setState({ board: null });
  useApp.setState({
    conversations: { a: conversation("a"), b: conversation("b"), c: conversation("c") },
    threads: {},
    selection: { type: "draft", kind: "chat" },
  });
  mockIPC((_command, payload) => {
    const req = (payload as { request: RequestOf<"getConversation"> }).request;
    if (req.method !== "getConversation") throw new Error(`not mocked: ${(req as { method: string }).method}`);
    const answer = { method: req.method, view: view(req.id) };
    if (!slow) return answer;
    return new Promise((resolve) => held.push(() => resolve(answer)));
  });
});

/** Lets the daemon's answers and what follows them land. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 20));
const shown = () => {
  const { selection } = useApp.getState();
  return selection.type === "conversation" ? selection.id : selection.type;
};
const run = () => useBoard.getState().board?.run;

function runChanged(id: string, state: RunState): EventEnvelope {
  return {
    seq: 1, stream: `conversation:${id}`, streamSeq: 1, atMs: 0,
    event: { type: "runStateChanged", conversationId: id, state, error: null },
  } as unknown as EventEnvelope;
}

test("a working session shown again shows it working at once, while its view is read again (issue 484)", async () => {
  select({ type: "conversation", id: "a" });
  await settle();
  assert.deepEqual([shown(), run()], ["a", "running"]);
  select({ type: "conversation", id: "b" });
  await settle();
  assert.deepEqual([shown(), run()], ["b", "idle"]);
  // The daemon is slow now: switching back must not wait for it to show the true state.
  slow = true;
  select({ type: "conversation", id: "a" });
  assert.deepEqual([shown(), run(), useBoard.getState().board?.loaded], ["a", "running", true]);
  assert.equal(useApp.getState().threads.a?.items.length, 2);
  // Coming back from Settings is the same.
  openSettings();
  assert.equal(useBoard.getState().board, null);
  select({ type: "conversation", id: "a" });
  assert.deepEqual([shown(), run()], ["a", "running"]);
  for (const answer of held) answer();
  await settle();
  assert.equal(run(), "running");
});

test("a session left keeps taking its events: shown again it shows where it got to", async () => {
  runs.a = "idle";
  select({ type: "conversation", id: "a" });
  await settle();
  select({ type: "conversation", id: "b" });
  await settle();
  // It started working while the user looked at another one.
  runs.a = "running";
  applyBoardEvents([runChanged("a", "running")]);
  slow = true;
  select({ type: "conversation", id: "a" });
  assert.deepEqual([shown(), run()], ["a", "running"]);
});

test("a session opened for the first time shows once its view is in, never empty or finished-looking (issues 484, 500)", async () => {
  select({ type: "conversation", id: "b" });
  await settle();
  slow = true;
  select({ type: "conversation", id: "c" });
  // Until the view is in, what was shown stays.
  await settle();
  assert.deepEqual([shown(), run()], ["b", "idle"]);
  for (const answer of held.splice(0)) answer();
  await settle();
  // It shows whole: its messages and its live state.
  assert.deepEqual([shown(), run(), useApp.getState().threads.c?.items.length], ["c", "running", 2]);
});

test("a session asked for while another still loads: only the newest one shows", async () => {
  slow = true;
  select({ type: "conversation", id: "a" });
  select({ type: "conversation", id: "c" });
  for (const answer of held.splice(0)) answer();
  await settle();
  assert.equal(shown(), "c");
  // A conversation asked for and then left for Settings never shows.
  select({ type: "conversation", id: "b" });
  openSettings();
  for (const answer of held.splice(0)) answer();
  await settle();
  assert.equal(shown(), "settings");
});
