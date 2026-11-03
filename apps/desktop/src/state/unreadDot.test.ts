import assert from "node:assert/strict";
import { test } from "node:test";

import type { Conversation, EventEnvelope } from "@/ipc/generated";
import { finishedAt, markSeen, migrateLastOpened, unread } from "@/state/lastOpened";
import { applyEvents, emptyThread, useApp } from "@/state/store";

const id = "chat";
const conversation = (extra: Record<string, unknown>) => ({ id, title: "Chat", updatedAtMs: 0, seenAtMs: 100, ...extra }) as unknown as Conversation;
const now = () => useApp.getState().conversations[id];
const dot = () => unread(finishedAt(now(), undefined), now()?.seenAtMs, false);

test("a request that finished while the conversation wasn't open marks it unread, until it is seen", () => {
  useApp.setState({ conversations: { [id]: conversation({}) }, threads: { [id]: emptyThread } });
  assert.equal(dot(), false);
  // A finish while the app listens.
  applyEvents([{ event: { type: "requestFinished", conversationId: id, requestId: "r1" }, atMs: 200, streamSeq: 2, seq: 2, stream: `conversation:${id}` } as EventEnvelope]);
  assert.equal(now()?.finishedAtMs, 200);
  assert.equal(dot(), true);
  markSeen(id, 200);
  assert.equal(dot(), false);
  // A finish missed while the app was disconnected: the catalog it reads on reconnecting says when.
  useApp.setState({ conversations: { [id]: conversation({ finishedAtMs: 400, seenAtMs: 200 }) } });
  assert.equal(dot(), true);
  markSeen(id, 400);
  assert.equal(dot(), false);
  markSeen(id, 300);
  assert.equal(now()?.seenAtMs, 400, "the seen clock only moves on");
  // Another window's seen event clears it here too.
  applyEvents([{ event: { type: "conversationSeen", id, atMs: 500 }, atMs: 600, streamSeq: 3, seq: 3, stream: "catalog" } as unknown as EventEnvelope]);
  assert.equal(now()?.seenAtMs, 500);
  // Open, nothing marks it.
  assert.equal(unread(500, 450, true), false);
});

test("one clock: the latest of a turn end, a finished request and an answer to another session", () => {
  assert.equal(finishedAt({ finishedAtMs: 10, peerAtMs: 30 }, 20), 30);
  assert.equal(finishedAt({ finishedAtMs: 40 }, 50), 50);
  assert.equal(finishedAt({}, undefined), undefined);
});

test("the old per-window opened times reach the daemon once", () => {
  useApp.setState({ conversations: { [id]: conversation({ seenAtMs: undefined, finishedAtMs: 300 }) } });
  const stored = new Map<string, string>([["brigadier.lastOpened", JSON.stringify({ [id]: 350, gone: 9 })]]);
  globalThis.localStorage = { getItem: (key: string) => stored.get(key) ?? null, removeItem: (key: string) => void stored.delete(key) } as Storage;
  migrateLastOpened(useApp.getState().conversations);
  assert.equal(now()?.seenAtMs, 350);
  assert.equal(stored.size, 0);
});

test("a seen time the daemon didn't take goes back, so the dot stays true and the next open retries", async () => {
  useApp.setState({ conversations: { [id]: conversation({ finishedAtMs: 300 }) } });
  // No daemon under node: the request fails.
  markSeen(id, 300);
  assert.equal(now()?.seenAtMs, 300, "cleared at once");
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(now()?.seenAtMs, 100);
  assert.equal(dot(), true);
});
