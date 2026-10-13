import assert from "node:assert/strict";
import { test } from "node:test";

import {
  peerLabel,
  peerNameOf,
  receivedCaption,
  receiverHasIt,
  refusedDescribed,
  replyOf,
  sentDescribed,
  stateLine,
  UNKNOWN_SESSION,
  waitingWords,
} from "@/app/conversation/activity/peerWords";
import type { Conversation, PeerMessage, PeerState, Project } from "@/ipc/generated";

const docs = { title: "Docs", project: "Handbook" };
const notes = { title: "Release notes", project: null };

const STATES: PeerState[] = [
  { type: "waitingForOk" }, { type: "pending" }, { type: "delivered" }, { type: "seen" }, { type: "replied" },
  { type: "declined" }, { type: "refused", reason: "Docs is archived." }, { type: "dropped", reason: "deleted" },
];

test("a sent message's row names the session, and its project when it's another, in every state", () => {
  const rows = STATES.map((state) => {
    const described = sentDescribed(state, docs);
    return [state.type, described.status, described.status === "inProgress" ? described.words.doing : described.words.done];
  });
  assert.deepEqual(rows, [
    ["waitingForOk", "completed", "Waiting for your OK to message Docs in Handbook"],
    ["pending", "inProgress", "Messaging Docs in Handbook"],
    ["delivered", "completed", "Messaged Docs in Handbook"],
    ["seen", "completed", "Messaged Docs in Handbook"],
    ["replied", "completed", "Messaged Docs in Handbook"],
    ["declined", "completed", "Didn’t message Docs in Handbook"],
    ["refused", "completed", "Couldn’t message Docs in Handbook"],
    ["dropped", "completed", "Couldn’t message Docs in Handbook"],
  ]);
  // Once it went, a pending row reads as sent.
  assert.equal(sentDescribed({ type: "pending" }, notes).words.done, "Messaged Release notes");
  assert.equal(sentDescribed({ type: "seen" }, notes).words.kind, "message");
  // A group of rows that didn't send says so in its summary.
  assert.equal(sentDescribed({ type: "declined" }, notes).words.phrase, "Didn’t message a session");
  assert.deepEqual(refusedDescribed(UNKNOWN_SESSION).words.done, "Couldn’t message a session");
});

test("the state line under a sent message, and when its name opens the message in the receiver", () => {
  assert.deepEqual(STATES.map((state) => stateLine(state).text), [
    "Waiting for your OK", "Sending", "Delivered", "Seen", "Replied", "You didn’t send it", "Docs is archived.",
    "Couldn’t deliver: the session was deleted",
  ]);
  assert.deepEqual(STATES.filter((state) => stateLine(state).reply).map((state) => state.type), ["replied"]);
  assert.equal(stateLine({ type: "dropped", reason: "archived" }).text, "Couldn’t deliver: the session was archived");
  assert.equal(stateLine({ type: "dropped", reason: "the daemon stopped" }).text, "Couldn’t deliver: the daemon stopped");
  // Only a message the receiver has opens it there.
  assert.deepEqual(STATES.filter(receiverHasIt).map((state) => state.type), ["pending", "delivered", "seen", "replied", "dropped"]);
});

test("a received message's caption, and a turn's words while it waits for replies", () => {
  assert.equal(receivedCaption(docs, false, false), "From Docs · Handbook");
  assert.equal(receivedCaption(notes, false, false), "From Release notes");
  assert.equal(receivedCaption(docs, true, false), "Reply from Docs");
  assert.equal(receivedCaption(docs, true, true), "Reply from Docs · to your earlier message");
  assert.equal(peerLabel(docs), "Docs in Handbook");
  assert.equal(peerLabel(notes), "Release notes");
  assert.equal(waitingWords([]), "Waiting for a reply");
  assert.equal(waitingWords(["Docs"]), "Waiting for Docs");
  assert.equal(waitingWords(["Docs", "Style guide"]), "Waiting for Docs and Style guide");
  assert.equal(waitingWords(["Docs", "Style guide", "Release notes"]), "Waiting for Docs, Style guide and Release notes");
});

const record = (messageId: string, outgoing: boolean, replyTo?: string): PeerMessage => ({
  messageId, outgoing, peer: "docs", requestId: "a1", state: { type: "seen" }, text: "", blob: null,
  sentAtMs: null, settled: false, position: 1, ...(replyTo && { replyTo }),
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
