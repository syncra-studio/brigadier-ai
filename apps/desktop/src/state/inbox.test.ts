import assert from "node:assert/strict";
import { test } from "node:test";

import type { Conversation } from "@/ipc/generated";
import { inboxLists, sortTime } from "@/state/inbox";

const row = (id: string, extra: Partial<Conversation> = {}): Conversation => ({
  id, kind: "session", projectId: "p", title: id, pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup: null,
  lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null, ...extra,
});
const ids = (list: Conversation[]) => list.map((conversation) => conversation.id);

test("Open is newest first by the user's last sent message, else created; nothing else counts", () => {
  const { open } = inboxLists([
    row("old", { createdAtMs: 10, lastSentAtMs: 100 }),
    row("sent-just-now", { createdAtMs: 5, lastSentAtMs: 500 }),
    // A finish, a peer's answer, a seen reply, sleep, wake and a rename move these clocks only.
    row("busy", { createdAtMs: 1, lastSentAtMs: 200, updatedAtMs: 9_000, finishedAtMs: 9_000, peerAtMs: 9_000, seenAtMs: 9_000 }),
    row("never-sent", { createdAtMs: 300 }),
  ]);
  assert.deepEqual(ids(open), ["sent-just-now", "never-sent", "busy", "old"]);
  assert.equal(sortTime(row("r", { createdAtMs: 7 })), 7);
});

test("pins sit above by their own time, archived rows dock apart, and a restored row keeps its sort time", () => {
  const rows = [
    row("pin-a", { pinnedAtMs: 1, lastSentAtMs: 900 }),
    row("pin-b", { pinnedAtMs: 2 }),
    row("newer", { lastSentAtMs: 300 }),
    row("put-away", { lifecycle: "archived", lastSentAtMs: 200, updatedAtMs: 5 }),
    row("put-away-later", { lifecycle: "archived", lastSentAtMs: 100, updatedAtMs: 8 }),
    row("older", { lastSentAtMs: 100 }),
    row("side", { sideOf: "newer" }),
  ];
  const lists = inboxLists(rows);
  assert.deepEqual(ids(lists.pinned), ["pin-b", "pin-a"]);
  assert.deepEqual(ids(lists.open), ["newer", "older"]);
  assert.deepEqual(ids(lists.archived), ["put-away-later", "put-away"]);
  const restored = inboxLists(rows.map((each) => (each.id === "put-away" ? { ...each, lifecycle: "active" as const } : each)));
  assert.deepEqual(ids(restored.open), ["newer", "put-away", "older"], "back where its sort time says");
});

test("a row stored as done or snoozed is back in Open, its old fields ignored; a fork stays under its source", () => {
  const old = { doneAtMs: 500, snoozedUntilMs: 9e12, snoozedAtMs: 500 } as unknown as Partial<Conversation>;
  const { open } = inboxLists([
    row("was-done", { createdAtMs: 50, ...old }),
    row("source", { lastSentAtMs: 40 }),
    row("fork", { lastSentAtMs: 900, forkedFrom: { conversationId: "source", messageId: "m" } }),
  ]);
  assert.deepEqual(ids(open), ["was-done", "source", "fork"]);
});
