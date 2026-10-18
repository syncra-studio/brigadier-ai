import assert from "node:assert/strict";
import { test } from "node:test";

import type { Conversation, EventEnvelope } from "@/ipc/generated";
import {
  applySettledTabEvents,
  rowTabs,
  stateWords,
  strongestState,
  tabState,
  useSettledTabs,
} from "@/state/tabStates";

const quiet = { running: false, awaiting: null, phase: null } as const;

test("a tab's state: needs you, then working, then ready or merged, else none", () => {
  assert.equal(tabState({ ...quiet, running: true, awaiting: "approval" }, false, "ready"), "needsYou");
  assert.equal(tabState(quiet, true, "merged"), "needsYou", "a quota wait needs the user");
  assert.equal(tabState({ ...quiet, phase: "watchingCi" }, false, "ready"), "working");
  assert.equal(tabState(quiet, false, "ready"), "ready");
  assert.equal(tabState(quiet, false, "merged"), "merged");
  assert.equal(tabState(quiet, false, undefined), null, "an untouched tab has none");
});

test("a workspace row takes its tabs' strongest state and counts the tabs sharing it", () => {
  assert.equal(strongestState([null, null]), null);
  assert.deepEqual(strongestState(["merged", "ready", null]), { state: "ready", count: 1 });
  assert.deepEqual(strongestState(["working", "needsYou", "needsYou"]), { state: "needsYou", count: 2 });
  assert.equal(stateWords({ state: "working", count: 2 }), "2 working");
  assert.equal(stateWords({ state: "needsYou", count: 2 }), "2 need you");
  assert.equal(stateWords({ state: "ready", count: 1 }), "Ready");
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

const tab = (id: string, workspaceId: string, extra: Partial<Conversation> = {}): Conversation => ({
  id, kind: "session", projectId: "p", title: id, pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup: null,
  lifecycle: "active", forkedFrom: null, sideOf: null, fallback: null, quotaWait: null, workspaceId, ...extra,
});

test("picked sidebar rows stand for their workspace's tabs, except pinned ones", () => {
  const conversations = Object.fromEntries([
    tab("a", "w1"), tab("b", "w1"), tab("pinned", "w1", { pinnedAtMs: 1 }), tab("gone", "w1", { lifecycle: "archived" }),
    tab("c", "w2"),
  ].map((each) => [each.id, each]));
  assert.deepEqual(rowTabs(conversations, ["a", "c"]).toSorted(), ["a", "b", "c"]);
  assert.deepEqual(rowTabs(conversations, ["pinned"]), ["pinned"], "a pinned tab's row is its own");
});
