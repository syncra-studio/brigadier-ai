import assert from "node:assert/strict";
import { test } from "node:test";

import type { Conversation, EventEnvelope } from "@/ipc/generated";
import { applySettledTabEvents, isTaskTab, tabState, useSettledTabs } from "@/state/tabStates";

const quiet = { running: false, awaiting: null, phase: null } as const;

test("a tab's state: needs you, then working, then ready or merged, else none", () => {
  assert.equal(tabState({ ...quiet, running: true, awaiting: "approval" }, false, "ready"), "needsYou");
  assert.equal(tabState(quiet, true, "merged"), "needsYou", "a quota wait needs the user");
  assert.equal(tabState({ ...quiet, phase: "watchingCi" }, false, "ready"), "working");
  assert.equal(tabState(quiet, false, "ready"), "ready");
  assert.equal(tabState(quiet, false, "merged"), "merged");
  assert.equal(tabState(quiet, false, undefined), null, "an untouched tab has none");
});

test("a request that settles ready marks its tab ready, and a merge of its work merged", () => {
  useSettledTabs.setState({ byTab: { old: "merged" } });
  let seq = 0;
  const envelope = (stream: string, event: EventEnvelope["event"]): EventEnvelope => ({ seq: ++seq, stream, streamSeq: seq, atMs: seq, event });
  applySettledTabEvents([
    envelope("conversation:a", { type: "requestReady", conversationId: "a", requestId: "r-1", head: "abc" }),
    envelope("conversation:b", { type: "requestReady", conversationId: "b", requestId: "r-2", head: "def" }),
    envelope("conversation:b", {
      type: "orchestratorStepped",
      step: { requestId: null, atMs: 1, position: 1, kind: { type: "merged", branch: "x", base: "main", commits: 1, askedIn: null } },
    }),
    envelope("conversation:old", { type: "requestReady", conversationId: "old", requestId: "r-3", head: "123" }),
  ]);
  assert.deepEqual(useSettledTabs.getState().byTab, { old: "ready", a: "ready", b: "merged" });
  const before = useSettledTabs.getState().byTab;
  applySettledTabEvents([envelope("conversation:a", { type: "requestReady", conversationId: "a", requestId: "r-4", head: "abd" })]);
  assert.equal(useSettledTabs.getState().byTab, before, "nothing new, nothing written");
});

const session = (extra: Partial<Conversation> = {}): Conversation => ({
  id: "s", kind: "session", projectId: "p", title: "s", pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup: null,
  lifecycle: "active", forkedFrom: null, sideOf: null, fallback: null, quotaWait: null, ...extra,
});

test("a project's open session is a task of its own; a chat, a side chat, an archived one aren't", () => {
  assert.equal(isTaskTab(session()), true);
  assert.equal(isTaskTab(session({ projectId: null })), false);
  assert.equal(isTaskTab(session({ kind: "chat" })), false);
  assert.equal(isTaskTab(session({ sideOf: "other" })), false);
  assert.equal(isTaskTab(session({ lifecycle: "archived" })), false);
});
