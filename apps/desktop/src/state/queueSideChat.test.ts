import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";
import { mockIPC } from "@tauri-apps/api/mocks";

import type { RequestOf } from "@/ipc/client";
import type { Conversation, Message, QueuedMessage } from "@/ipc/generated";

const data = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => data.get(key) ?? null,
  setItem: (key: string, value: string) => data.set(key, value),
  removeItem: (key: string) => data.delete(key),
} });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage } });
const { openQueuedInSideChat } = await import("./actions");
const { emptyBoard, useBoard } = await import("./board");
const { sessionTabs, useSessionTabs } = await import("./sessionTabs");
const { emptyThread, useApp } = await import("./store");

const parent = "session-1";
const item: QueuedMessage = { id: "queued", text: "Why is the build slow?", attachments: [], mentions: [], queuedAtMs: 20, editedAtMs: null, deciding: false };
const other: QueuedMessage = { ...item, id: "first", text: "Also run the tests" };

type Sent = RequestOf<"deleteQueued"> | RequestOf<"openSideChat"> | RequestOf<"sendMessage">;
let sent: Sent[];
/** What the daemon does with each request; a throw is its error. */
let daemon: { deleteQueued: () => void; sendSide: () => void };

const side = (id: string) => ({ id, kind: "chat", title: "", sideOf: parent }) as unknown as Conversation;
const message = (conversationId: string): Message => ({
  id: "m1", conversationId, seq: 1, role: "user", text: item.text, attachments: [], mentions: [], createdAtMs: 30,
  blob: null, model: null, requestId: "m1", parentId: null,
});

beforeEach(() => {
  sent = [];
  daemon = { deleteQueued: () => {}, sendSide: () => {} };
  useSessionTabs.setState({ sessions: {} });
  useApp.setState({ selection: { type: "conversation", id: parent }, pending: [], threads: { [parent]: emptyThread } });
  useBoard.setState({ board: { ...emptyBoard(parent), run: "running", queue: { items: [other, item], paused: false } } });
  mockIPC((_command, payload) => {
    const req = (payload as { request: Sent }).request;
    sent.push(req);
    switch (req.method) {
      case "deleteQueued":
        daemon.deleteQueued();
        return { method: req.method, queue: { items: [other], paused: false } };
      case "openSideChat":
        return { method: req.method, conversation: side(req.sideChatId!) };
      case "sendMessage":
        if (req.conversationId !== parent) daemon.sendSide();
        return req.conversationId === parent
          ? { method: req.method, outcome: { type: "queued", item } }
          : { method: req.method, outcome: { type: "sent", message: message(req.conversationId) } };
      default:
        throw new Error(`not mocked: ${(req as { method: string }).method}`);
    }
  });
});

const sideTabs = () => sessionTabs(parent).tabs.flatMap((tab) => (tab.kind === "sideChat" ? [tab] : []));

test("a queued message leaves the queue before it goes to a new side chat, whose tab opens on it", async () => {
  assert.equal(await openQueuedInSideChat(parent, item), true);
  assert.deepEqual(sent.map((req) => req.method), ["deleteQueued", "openSideChat", "sendMessage"]);
  const [, open, send] = sent as [RequestOf<"deleteQueued">, RequestOf<"openSideChat">, RequestOf<"sendMessage">];
  assert.equal(open.conversationId, parent);
  assert.equal(send.conversationId, open.sideChatId);
  assert.equal(send.text, item.text);
  const [tab] = sideTabs();
  assert.equal(tab?.conversationId, open.sideChatId);
  assert.equal(sessionTabs(parent).active, tab?.id);
});

test("a message the turn already took goes nowhere else: no side chat, no send, no tab", async () => {
  daemon.deleteQueued = () => {
    throw { kind: "notFound", message: "queued message queued" };
  };
  assert.equal(await openQueuedInSideChat(parent, item), false);
  assert.equal(sent.some((req) => req.method === "openSideChat" || req.method === "sendMessage"), false);
  assert.deepEqual(sideTabs(), []);
});

test("a side chat that can't take the message puts it back in its slot of the queue", async () => {
  daemon.sendSide = () => {
    throw { kind: "invalid", message: "the side chat is gone" };
  };
  await assert.rejects(openQueuedInSideChat(parent, item));
  const back = sent.at(-1) as RequestOf<"sendMessage">;
  assert.equal(back.method, "sendMessage");
  assert.equal(back.conversationId, parent);
  assert.equal(back.text, item.text);
  assert.equal(back.steer, false);
  assert.equal(back.queueIndex, 1);
  assert.deepEqual(sideTabs(), []);
});
