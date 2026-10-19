import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import type { Conversation, ConversationActivity, EventEnvelope, TabState } from "@/ipc/generated";
import { fromSnapshot, rowActivity } from "@/state/activity";
import { applySettledTabEvents, isTaskTab, sessionStatus, tabState, useSettledTabs } from "@/state/tabStates";

const quiet = { running: false, awaiting: null, phase: null, workers: 0 } as const;

type Case = { name: string; activity: ConversationActivity | null; quotaWait: boolean; settled: "ready" | "merged" | "reverted" | null; expect: TabState | null };

test("the app reads each case of the states table as the engine does (crates/core/src/session_states.json)", () => {
  const cases = JSON.parse(readFileSync(new URL("../../../../crates/core/src/session_states.json", import.meta.url), "utf8")) as Case[];
  assert.ok(cases.length >= 10);
  for (const each of cases) {
    const activity = rowActivity(each.activity ? fromSnapshot(each.activity) : undefined);
    assert.equal(tabState(activity, each.quotaWait, each.settled ?? undefined), each.expect, each.name);
  }
});

test("a session's state comes with the reason its row names", () => {
  const reset = new Date(2026, 9, 18, 14, 5).getTime();
  assert.deepEqual(sessionStatus({ ...quiet, running: true, phase: "watchingCi", workers: 2 }, null, undefined), { state: "working", reason: "watching CI" });
  // How many workers it runs is never its reason: workers show in the session, not its row or tab.
  assert.deepEqual(sessionStatus({ ...quiet, running: true, workers: 2 }, null, undefined), { state: "working", reason: null });
  assert.deepEqual(sessionStatus({ ...quiet, running: true }, null, undefined), { state: "working", reason: null });
  assert.deepEqual(sessionStatus(quiet, { resetsAtMs: reset }, { state: "ready" }), { state: "working", reason: "waiting for reset 14:05" });
  assert.deepEqual(sessionStatus(quiet, { resetsAtMs: null }, undefined), { state: "working", reason: "waiting for a usage limit" });
  assert.deepEqual(sessionStatus({ ...quiet, awaiting: "approval" }, null, undefined), { state: "needsYou", reason: "approval" });
  assert.deepEqual(sessionStatus({ ...quiet, awaiting: "input" }, null, undefined), { state: "needsYou", reason: "question" });
  assert.deepEqual(sessionStatus({ ...quiet, awaiting: "failed" }, null, undefined), { state: "failed", reason: null });
  assert.deepEqual(sessionStatus({ ...quiet, running: true, phase: "merging" }, null, { state: "ready" }), { state: "merging", reason: null });
  assert.deepEqual(sessionStatus(quiet, null, { state: "ready", pullRequest: "open", proof: true }), { state: "prOpen", reason: "proof attached" });
  assert.deepEqual(sessionStatus(quiet, null, { state: "ready", pullRequest: "closed" }), { state: "ready", reason: null });
  assert.deepEqual(sessionStatus(quiet, null, { state: "merged", proof: true }), { state: "merged", reason: null });
  assert.deepEqual(sessionStatus(quiet, null, { state: "reverted" }), { state: "reverted", reason: null });
  assert.deepEqual(sessionStatus(quiet, null, { pullRequest: "open" }), { state: "prOpen", reason: null });
  assert.deepEqual(sessionStatus(quiet, null, { state: "ready", pullRequest: "draft" }), { state: "prOpen", reason: null });
  assert.deepEqual(sessionStatus(quiet, null, undefined, true), { state: "stopped", reason: null });
  assert.deepEqual(sessionStatus({ ...quiet, running: true }, null, undefined, true), { state: "working", reason: null });
  assert.deepEqual(sessionStatus({ ...quiet, awaiting: "approval" }, null, undefined, true), { state: "needsYou", reason: "approval" });
  assert.deepEqual(sessionStatus({ ...quiet, awaiting: "failed" }, { resetsAtMs: null }, undefined), { state: "working", reason: "waiting for a usage limit" });
  assert.deepEqual(sessionStatus(quiet, null, { state: "merged", pullRequest: "open" }), { state: "merged", reason: null });
});

test("a request that settles ready marks its session ready, a merge merged, and proof and pull requests come along", () => {
  useSettledTabs.setState({ byTab: { old: { state: "merged" } } });
  let seq = 0;
  const envelope = (stream: string, event: EventEnvelope["event"]): EventEnvelope => ({ seq: ++seq, stream, streamSeq: seq, atMs: seq, event });
  applySettledTabEvents([
    envelope("conversation:a", { type: "requestReady", conversationId: "a", requestId: "r-1", head: "abc" }),
    envelope("conversation:a", { type: "pullRequestSeen", conversationId: "a", pullRequest: { number: 7, title: "A", url: "https://github.com/o/n/pull/7", state: "open" } }),
    envelope("conversation:a", {
      type: "orchestratorStepped",
      step: { requestId: "r-1", atMs: 1, position: 1, kind: { type: "proof", id: "p1", name: "shot.png", mime: "image/png" } },
    }),
    envelope("conversation:b", { type: "requestReady", conversationId: "b", requestId: "r-2", head: "def" }),
    envelope("conversation:b", {
      type: "orchestratorStepped",
      step: { requestId: null, atMs: 1, position: 2, kind: { type: "merged", branch: "x", base: "main", commits: 1, askedIn: null } },
    }),
    envelope("conversation:old", { type: "requestReady", conversationId: "old", requestId: "r-3", head: "123" }),
  ]);
  assert.deepEqual(useSettledTabs.getState().byTab, {
    old: { state: "ready" },
    a: { state: "ready", pullRequest: "open", proof: true },
    b: { state: "merged", proof: false },
  });
  const before = useSettledTabs.getState().byTab;
  applySettledTabEvents([envelope("conversation:a", { type: "requestReady", conversationId: "a", requestId: "r-4", head: "abd" })]);
  assert.equal(useSettledTabs.getState().byTab, before, "nothing new, nothing written");
  applySettledTabEvents([envelope("conversation:a", { type: "pullRequestSeen", conversationId: "a", pullRequest: null })]);
  assert.deepEqual(useSettledTabs.getState().byTab.a, { state: "ready", proof: true }, "no pull request any more");
});

test("a failed turn needs the user until the next one runs", () => {
  const failed = fromSnapshot({ conversationId: "s", run: "idle", tasks: [], approvals: [], questions: [], phases: [], failed: true });
  assert.equal(rowActivity(failed).awaiting, "failed");
  assert.equal(rowActivity({ ...failed, run: "running" }).awaiting, null);
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
