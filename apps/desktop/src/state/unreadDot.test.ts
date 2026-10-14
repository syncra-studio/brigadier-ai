import assert from "node:assert/strict";
import { test } from "node:test";

import type { Conversation, EventEnvelope } from "@/ipc/generated";
import { lastFinished, markOpened, unread, useLastOpened } from "@/state/lastOpened";
import { applyEvents, emptyThread, useApp } from "@/state/store";

const id = "chat";
const conversation = (extra: Partial<Conversation>) => ({ id, title: "Chat", updatedAtMs: 0, ...extra }) as unknown as Conversation;
const dot = () => unread(lastFinished(useApp.getState().conversations[id]), useLastOpened.getState().at[id], false);

test("a request that finished while the conversation wasn't open marks it unread, until it is opened", () => {
  useLastOpened.setState({ at: { [id]: 100 } });
  useApp.setState({ conversations: { [id]: conversation({}) }, threads: { [id]: emptyThread } });
  assert.equal(dot(), false);
  // A finish while the app listens.
  applyEvents([{ event: { type: "requestFinished", conversationId: id, requestId: "r1" }, atMs: 200, streamSeq: 2, seq: 2, stream: `conversation:${id}` } as EventEnvelope]);
  assert.equal(useApp.getState().conversations[id]?.finishedAtMs, 200);
  assert.equal(dot(), true);
  markOpened(id, 250);
  assert.equal(dot(), false);
  // A finish missed while the app was disconnected: the catalog it reads on reconnecting says when.
  // (what `replaceCatalog` sets from it; its settings half needs a document)
  useApp.setState({ conversations: { [id]: conversation({ finishedAtMs: 400 }) } });
  assert.equal(dot(), true);
  markOpened(id, 450);
  assert.equal(dot(), false);
  // Open, nothing marks it.
  assert.equal(unread(500, 450, true), false);
});

test("the dot takes the later of a finished request and an answer to another session", () => {
  assert.equal(lastFinished({ finishedAtMs: 10, peerAtMs: 30 }), 30);
  assert.equal(lastFinished({ finishedAtMs: 40 }), 40);
  assert.equal(lastFinished({}), undefined);
});
