import assert from "node:assert/strict";
import { test } from "node:test";

import type { Conversation, EventEnvelope, Message } from "@/ipc/generated";
import { applyEvents, emptyThread, useApp } from "@/state/store";

const id = "docs";
const envelope = (event: EventEnvelope["event"], atMs: number): EventEnvelope =>
  ({ event, streamSeq: atMs, atMs, seq: atMs, stream: `conversation:${id}` }) as EventEnvelope;

test("a session's unread dot comes when it finished what another session asked, not when the message came", () => {
  useApp.setState({
    conversations: { [id]: { id, title: "Docs", updatedAtMs: 0, peerAtMs: null } as unknown as Conversation },
    threads: { [id]: emptyThread },
  });
  const message = { id: "m1", conversationId: id, role: "peer", text: "Is the guide ready?", createdAtMs: 10, requestId: "m1" } as Message;
  applyEvents([envelope({ type: "messageAppended", message } as EventEnvelope["event"], 10)]);
  assert.equal(useApp.getState().conversations[id]?.peerAtMs ?? null, null);
  applyEvents([envelope({ type: "peerAnswered", conversationId: id, messageId: "m1" }, 20)]);
  assert.equal(useApp.getState().conversations[id]?.peerAtMs, 20);
  // An older answer read again leaves the newer mark.
  applyEvents([envelope({ type: "peerAnswered", conversationId: id, messageId: "m0" }, 15)]);
  assert.equal(useApp.getState().conversations[id]?.peerAtMs, 20);
});
