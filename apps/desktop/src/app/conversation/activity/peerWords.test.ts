import assert from "node:assert/strict";
import { test } from "node:test";

import {
  type Exchange,
  exchangeDescribed,
  exchangeWords,
  type PeerRef,
  type PeerSend,
  peerNameOf,
  plainWords,
  receiverHasIt,
  refusedDescribed,
  replyOf,
  sentByCaption,
  UNKNOWN_SESSION,
} from "@/app/conversation/activity/peerWords";
import type { Conversation, ItemStatus, PeerMessage, PeerState, Project } from "@/ipc/generated";

const docs: PeerRef = { session: "docs", title: "Docs" };
const notes: PeerRef = { session: "notes", title: "Release notes" };

const STATES: PeerState[] = [
  { type: "waitingForOk" }, { type: "pending" }, { type: "delivered" }, { type: "seen" }, { type: "replied" },
  { type: "declined" }, { type: "refused", reason: "Docs is archived." }, { type: "dropped", reason: "deleted" },
];

const send = (ref: PeerRef, type: PeerState["type"]): PeerSend => ({ ...ref, state: { type } as PeerState });
const exchange = (sends: PeerSend[], waits: ItemStatus[] = [], received: PeerRef[] = [], waitingOn: PeerRef[] = []): Exchange => ({
  sends, waits, received, waitingOn,
});
/** Just sent, with nothing after it yet in a running turn. */
const going = (value: Exchange): Exchange => ({ ...value, going: true });
const line = (value: Exchange) => {
  const words = exchangeWords(value);
  return [plainWords(words.parts), words.status, words.waiting];
};

test("a row of one message reads how it stands, from sending to waiting to the finished exchange", () => {
  assert.deepEqual(STATES.map((state) => [state.type, ...line(exchange([send(docs, state.type)]))]), [
    ["waitingForOk", "Waiting for your OK to message Docs", "completed", false],
    ["pending", "Sending message to Docs", "inProgress", false],
    ["delivered", "Sent message to Docs", "completed", false],
    ["seen", "Sent message to Docs", "completed", false],
    ["replied", "Sent message to Docs", "completed", false],
    ["declined", "Didn’t send message to Docs", "completed", false],
    ["refused", "Couldn’t send message to Docs", "completed", false],
    ["dropped", "Couldn’t send message to Docs", "completed", false],
  ]);
  // Once it went, the same row waits for the receiver, then names the reply it waited for.
  assert.deepEqual(line(exchange([send(docs, "delivered")], ["inProgress"])), ["Waiting for Docs to reply", "inProgress", true]);
  assert.deepEqual(line(exchange([send(docs, "replied")], ["completed"], [docs])), ["Sent message to Docs, waited for its reply", "completed", false]);
  assert.deepEqual(line(exchange([send(docs, "seen")], ["completed"])), ["Sent message to Docs, waited; no reply yet", "completed", false]);
  assert.deepEqual(line(exchange([send(docs, "seen")], ["failed"])), ["Sent message to Docs, couldn’t wait for a reply", "completed", false]);
});

test("a message just sent, with nothing after it yet in a running turn, still reads as sending", () => {
  assert.deepEqual(line(going(exchange([send(docs, "delivered")]))), ["Sending message to Docs", "inProgress", false]);
  assert.deepEqual(line(going(exchange([send(docs, "seen"), send(notes, "seen")]))), ["Sending messages to Docs and Release notes", "inProgress", false]);
  // Its wait takes the row over.
  assert.deepEqual(line(going(exchange([send(docs, "delivered")], ["inProgress"]))), ["Waiting for Docs to reply", "inProgress", true]);
  // One that waits for the user's OK, or didn't go, says so.
  assert.deepEqual(line(going(exchange([send(docs, "waitingForOk")]))), ["Waiting for your OK to message Docs", "completed", false]);
  assert.deepEqual(line(going(exchange([send(docs, "seen"), send(notes, "refused")]))), ["Sent message to Docs, couldn’t send message to Release notes", "completed", false]);
});

test("a row of several messages names each session once, and only the replies that came", () => {
  const both = [send(docs, "replied"), send(notes, "seen")];
  assert.deepEqual(line(exchange(both)), ["Sent messages to Docs and Release notes", "completed", false]);
  assert.deepEqual(line(exchange([send(docs, "pending"), send(notes, "pending")])), ["Sending messages to Docs and Release notes", "inProgress", false]);
  // While one is still due, the row names the one it waits for.
  assert.deepEqual(line(exchange(both, ["completed", "inProgress"], [docs], [notes])), ["Waiting for Release notes to reply", "inProgress", true]);
  assert.deepEqual(line(exchange(both, ["completed"], [docs])), ["Sent messages to Docs and Release notes, waited for a reply from Docs", "completed", false]);
  assert.deepEqual(line(exchange([send(docs, "replied"), send(notes, "replied")], ["completed", "completed"], [docs, notes])), [
    "Sent messages to Docs and Release notes, waited for their replies", "completed", false,
  ]);
  // Two messages to one session are one name.
  assert.deepEqual(line(exchange([send(docs, "seen"), send(docs, "seen")])), ["Sent messages to Docs", "completed", false]);
  // A mix of sent and refused says both.
  assert.deepEqual(line(exchange([send(docs, "seen"), send(notes, "refused")])), ["Sent message to Docs, couldn’t send message to Release notes", "completed", false]);
});

test("a wait with no message next to it is a row of its own", () => {
  assert.deepEqual(line(exchange([], ["inProgress"], [], [docs])), ["Waiting for Docs to reply", "inProgress", true]);
  assert.deepEqual(line(exchange([], ["inProgress"])), ["Waiting for replies", "inProgress", true]);
  assert.deepEqual(line(exchange([], ["completed"], [docs])), ["Waited for a reply from Docs", "completed", false]);
  assert.deepEqual(line(exchange([], ["completed"], [docs, notes])), ["Waited for replies from Docs and Release notes", "completed", false]);
  assert.deepEqual(line(exchange([], ["completed"])), ["Waited; no reply yet", "completed", false]);
});

test("the row's words as a group reads them: kind, summary phrase and the waiting clock", () => {
  const waiting = exchangeDescribed(exchange([send(docs, "seen")], ["inProgress"]));
  assert.deepEqual(waiting, {
    words: { kind: "message", doing: "Waiting for Docs to reply", done: "Waiting for Docs to reply", object: "Docs", waiting: true },
    status: "inProgress",
  });
  const refused = exchangeDescribed(exchange([send(docs, "refused")]));
  assert.equal(refused.words.kind, "other");
  assert.equal(refused.words.phrase, "Couldn’t message a session");
  // A message waiting for the user's OK counts as sent for the group.
  assert.equal(exchangeDescribed(exchange([send(docs, "waitingForOk")])).words.kind, "message");
  assert.deepEqual(refusedDescribed(UNKNOWN_SESSION).words.done, "Couldn’t send message to a session");
});

test("only a message the receiver has opens it there", () => {
  assert.deepEqual(STATES.filter(receiverHasIt).map((state) => state.type), ["pending", "delivered", "seen", "replied", "dropped"]);
});

test("a received message's caption", () => {
  assert.equal(sentByCaption({ title: "API work", project: "Handbook" }), "Sent by API work from another session");
});

const record = (messageId: string, outgoing: boolean, replyTo?: string): PeerMessage => ({
  messageId, outgoing, peer: "docs", requestId: "a1", state: { type: "seen" }, text: "", blob: null,
  sentAtMs: null, settled: false, position: 1, automatic: false, ...(replyTo && { replyTo }),
});
const conversation = (id: string, projectId: string | null, title: string) => ({ id, projectId, title }) as Conversation;

test("a sent message's reply is the received record naming it", () => {
  const records = { m1: record("m1", true), r1: record("r1", false, "m1"), o1: record("o1", true, "m1") };
  assert.equal(replyOf(records.m1, records)?.messageId, "r1");
  assert.equal(replyOf(records.r1, records), null);
});

test("another session is named by its title, with its project only when it's another one", () => {
  const conversations = { docs: conversation("docs", "handbook", "Docs"), notes: conversation("notes", "website", "Release notes"), chat: conversation("chat", null, "A chat") };
  const projects = { handbook: { id: "handbook", name: "Handbook" } as Project, website: { id: "website", name: "Website" } as Project };
  assert.deepEqual(peerNameOf(conversations, projects, "docs", "website"), { title: "Docs", project: "Handbook" });
  assert.deepEqual(peerNameOf(conversations, projects, "notes", "website"), { title: "Release notes", project: null });
  assert.deepEqual(peerNameOf(conversations, projects, "chat", "website"), { title: "A chat", project: null });
  // A session the app no longer has.
  assert.equal(peerNameOf(conversations, projects, "gone", "website"), null);
  assert.equal(peerNameOf(conversations, projects, undefined, "website"), null);
});
