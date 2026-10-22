import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import type { Conversation, EventEnvelope, McpStatus, RunState, TabState, TaskState, TurnEnd } from "@/ipc/generated";
import { applyActivityEvents, applyActivityNews, fromSnapshot, rowActivity, stoppedOf, useActivity } from "@/state/activity";
import { applySettledTabEvents, isTaskTab, sessionStatus, sidebarIndicators, tabState, useSettledTabs } from "@/state/tabStates";

const quiet = { running: false, awaiting: null, workers: 0 } as const;

type Case = {
  name: string;
  turn?: { open?: boolean; lastEnd?: TurnEnd };
  run?: RunState;
  tasks?: [string, TaskState][];
  approvals?: string[];
  plans?: string[];
  questions?: string[];
  servers?: { name: string; status: McpStatus }[];
  quotaWait?: boolean;
  settled?: "ready" | "merged" | "reverted" | null;
  expect: TabState | null;
  status: string | null;
  reason?: string;
};

/** A case's activity as the app folds it: the run, the CLI's turn news, the cards, the servers. */
function activityOf(each: Case, id: string) {
  useActivity.setState({ byConversation: {} });
  const on = (event: unknown) => ({ stream: `conversation:${id}`, event }) as EventEnvelope;
  if (each.turn?.lastEnd) {
    applyActivityNews({ conversationId: id, entries: [], opened: true });
    applyActivityNews({ conversationId: id, entries: [], ended: each.turn.lastEnd });
  }
  if (each.turn?.open) applyActivityNews({ conversationId: id, entries: [], opened: true });
  applyActivityEvents([
    on({ type: "runStateChanged", conversationId: id, state: each.run ?? "idle" }),
    ...(each.tasks ?? []).map(([task, state]) => on({ type: "taskUpdated", task: { id: task, conversationId: id, kind: "write", state, title: task } })),
    ...(each.approvals ?? []).map((card) => on({ type: "approvalUpdated", approval: { id: card, conversationId: id, state: { type: "pending" } } })),
    ...(each.plans ?? []).map((card) => on({ type: "planUpdated", plan: { id: card, conversationId: id, state: { type: "proposed" } } })),
    ...(each.questions ?? []).map((card) => on({ type: "questionUpdated", question: { id: card, conversationId: id, answer: null } })),
    on({ type: "mcpServersUpdated", conversationId: id, taskId: null, provider: "claude", atStart: false, boot: "b",
      servers: (each.servers ?? []).map((server) => ({ ...server, tools: [], transport: null })) }),
  ]);
  return useActivity.getState().byConversation[id];
}

test("the app reads each case of the sidebar's states table as the engine does (crates/core/src/sidebar_states.json)", () => {
  const cases = JSON.parse(readFileSync(new URL("../../../../crates/core/src/sidebar_states.json", import.meta.url), "utf8")) as Case[];
  assert.ok(cases.length >= 20);
  for (const [index, each] of cases.entries()) {
    const activity = activityOf(each, `case-${index}`);
    const row = rowActivity(activity);
    const settled = each.settled ?? undefined;
    assert.equal(tabState(row, !!each.quotaWait, settled), each.expect, each.name);
    const status = sessionStatus(row, each.quotaWait ? { resetsAtMs: null } : null, settled ? { state: settled } : undefined, stoppedOf(activity));
    assert.equal(status?.state ?? null, each.status, `${each.name}: its words`);
    if (each.reason) assert.equal(status?.reason, each.reason, `${each.name}: its reason`);
  }
});

test("a session's state comes with the reason its row names", () => {
  const reset = new Date(2026, 9, 18, 14, 5).getTime();
  // How many workers it runs is never its reason: workers show in the session, not its row or tab.
  assert.deepEqual(sessionStatus({ ...quiet, running: true, workers: 2 }, null, undefined), { state: "working", reason: null });
  assert.deepEqual(sessionStatus({ ...quiet, running: true }, null, undefined), { state: "working", reason: null });
  assert.deepEqual(sessionStatus(quiet, { resetsAtMs: reset }, { state: "ready" }), { state: "working", reason: "waiting for reset 14:05" });
  assert.deepEqual(sessionStatus(quiet, { resetsAtMs: null }, undefined), { state: "working", reason: "waiting for a usage limit" });
  assert.deepEqual(sessionStatus({ ...quiet, awaiting: "approval" }, null, undefined), { state: "needsYou", reason: "approval" });
  assert.deepEqual(sessionStatus({ ...quiet, awaiting: "input" }, null, undefined), { state: "needsYou", reason: "question" });
  assert.deepEqual(sessionStatus({ ...quiet, awaiting: "failed" }, null, undefined), { state: "failed", reason: null });
  assert.deepEqual(sessionStatus({ ...quiet, awaiting: "signIn" }, null, undefined), { state: "needsYou", reason: "sign-in" });
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
  useSettledTabs.setState({ byTab: { old: { state: "merged" }, b: { state: "ready", ahead: 2 } } });
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
  }, "a merge drops the old count of commits ahead until git counts again");
  const before = useSettledTabs.getState().byTab;
  applySettledTabEvents([envelope("conversation:a", { type: "requestReady", conversationId: "a", requestId: "r-4", head: "abd" })]);
  assert.equal(useSettledTabs.getState().byTab, before, "nothing new, nothing written");
  applySettledTabEvents([envelope("conversation:a", { type: "pullRequestSeen", conversationId: "a", pullRequest: null })]);
  assert.deepEqual(useSettledTabs.getState().byTab.a, { state: "ready", proof: true }, "no pull request any more");
});

test("a failed turn needs the user until the next one runs", () => {
  const failed = fromSnapshot({ conversationId: "s", run: "idle", tasks: [], approvals: [], questions: [], failed: true });
  assert.equal(rowActivity(failed).awaiting, "failed");
  assert.equal(rowActivity({ ...failed, turnOpen: true }).awaiting, null);
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


test("sidebar marks follow live work, errors and unread activity independently of settled state words", () => {
  assert.deepEqual(sidebarIndicators(quiet, false, false), { working: false, error: false, unread: false });
  assert.deepEqual(sidebarIndicators(quiet, false, true), { working: false, error: false, unread: true });
  assert.deepEqual(sidebarIndicators({ ...quiet, running: true }, false, true), { working: true, error: false, unread: false });
  assert.deepEqual(sidebarIndicators(quiet, true, true), { working: true, error: false, unread: false });
  assert.deepEqual(sidebarIndicators({ ...quiet, awaiting: "approval" }, false, false), { working: false, error: false, unread: false });
  assert.deepEqual(sidebarIndicators({ ...quiet, awaiting: "approval", running: true }, false, true), { working: true, error: false, unread: false });
  assert.deepEqual(sidebarIndicators({ ...quiet, awaiting: "input", running: true }, false, true), { working: false, error: false, unread: true });
  assert.deepEqual(sidebarIndicators({ ...quiet, awaiting: "failed" }, false, true), { working: false, error: true, unread: true });
  assert.deepEqual(sidebarIndicators({ ...quiet, awaiting: "failed", running: true }, false, true), { working: true, error: false, unread: false });
});
