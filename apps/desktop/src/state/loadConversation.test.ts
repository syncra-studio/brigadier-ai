import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";
import { mockIPC } from "@tauri-apps/api/mocks";

import type { RequestOf } from "@/ipc/client";
import type { Conversation, ConversationView, Message } from "@/ipc/generated";

const data = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => data.get(key) ?? null,
  setItem: (key: string, value: string) => data.set(key, value),
  removeItem: (key: string) => data.delete(key),
} });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage } });
const { loadConversation, loadEarlier } = await import("./actions");
const { useApp } = await import("./store");

const ID = "old-conversation";
const conversation = { id: ID, kind: "chat", title: "Old", sideOf: null } as unknown as Conversation;
const message = (seq: number): Message => ({
  id: `m${seq}`, conversationId: ID, seq, role: seq % 2 ? "user" : "assistant", text: `message ${seq}`, attachments: [], mentions: [],
  createdAtMs: seq, blob: null, model: null, requestId: null, parentId: null,
});
/** The daemon's history: 500 messages, seq 1 to 500. */
let history: Message[];
let asked: string[];

/** The newest `limit` messages before `before`, oldest first, as the daemon pages them. */
function pageOf(before: number | null, limit: number) {
  const older = history.filter((each) => before === null || each.seq < before);
  return { messages: older.slice(-limit), hasMore: older.length > limit };
}

beforeEach(() => {
  history = Array.from({ length: 500 }, (_, index) => message(index + 1));
  asked = [];
  useApp.setState({ conversations: { [ID]: conversation }, threads: {} });
  mockIPC((_command, payload) => {
    const req = (payload as { request: RequestOf<"getConversation"> | RequestOf<"listMessages"> }).request;
    if (req.method === "getConversation") {
      asked.push("view");
      const view = {
        conversation, messages: pageOf(null, req.limit ?? 200), tasks: [], approvals: [], questions: [], requests: [],
        workerSteps: [], orchestratorSteps: [], decisions: [], compactions: [], ratings: {}, notices: [], memories: [],
      } as unknown as ConversationView;
      return { method: req.method, view };
    }
    if (req.method === "listMessages") {
      asked.push(`before ${req.before}`);
      return { method: req.method, page: pageOf(req.before ?? null, req.limit ?? 200) };
    }
    throw new Error(`not mocked: ${(req as { method: string }).method}`);
  });
});

const seqs = () => useApp.getState().threads[ID]!.items.map((each) => each.seq);

test("reopening a conversation keeps the history it already paged back through", async () => {
  await loadConversation(ID);
  await loadEarlier(ID);
  assert.deepEqual([seqs()[0], seqs().length, useApp.getState().threads[ID]!.hasMore], [101, 400, true]);
  // Two messages came live meanwhile, and the conversation is opened again.
  history.push(message(501), message(502));
  useApp.setState(({ threads }) => ({ threads: { ...threads, [ID]: { ...threads[ID]!, items: [...threads[ID]!.items, message(501), message(502)] } } }));
  asked = [];
  await loadConversation(ID);
  // Only the view is read again: the older pages stay, joined to the new one with no gap.
  assert.deepEqual(asked, ["view"]);
  assert.deepEqual([seqs()[0], seqs().at(-1), seqs().length, useApp.getState().threads[ID]!.hasMore], [101, 502, 402, true]);
  assert.ok(seqs().every((seq, index) => index === 0 || seq === seqs()[index - 1]! + 1));
  // Paging back again goes on from where it was.
  await loadEarlier(ID);
  assert.deepEqual(asked, ["view", "before 101"]);
  assert.equal(seqs()[0], 1);
});

test("after missed events only the fresh page is kept, and a thread that can't join keeps only its page", async () => {
  await loadConversation(ID);
  await loadEarlier(ID);
  await loadConversation(ID, { fresh: true });
  assert.deepEqual([seqs()[0], seqs().length, useApp.getState().threads[ID]!.hasMore], [301, 200, true]);
  // Held history that no longer reaches the newest page (300 messages came since, not seen live).
  await loadEarlier(ID);
  history.push(...Array.from({ length: 300 }, (_, index) => message(501 + index)));
  await loadConversation(ID);
  assert.deepEqual([seqs()[0], seqs().length], [601, 200]);
});
