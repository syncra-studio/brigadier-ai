import assert from "node:assert/strict";
import { test } from "node:test";

import type { Conversation, EventEnvelope } from "@/ipc/generated";
import { fromSnapshot, rowActivity } from "@/state/activity";
import { useApp } from "@/state/store";
import { TAB_STATES, TAB_STATE_WORDS, SESSION_STATUS_WORDS, applySettledTabEvents, isTaskTab, nothingToMerge, sessionStatus, settledByTab, sidebarIndicators, tabState, useSettledTabs } from "@/state/tabStates";

const quiet = { running: false, awaiting: null, workers: 0 } as const;

test("session states use only activity, in the requested precedence order", () => {
  const complete = true;
  assert.equal(tabState(quiet, false), null);
  assert.equal(tabState(quiet, false, false, complete), "done");
  assert.equal(tabState(quiet, false, true, complete), "stopped");
  assert.equal(tabState({ ...quiet, running: true }, false, true, complete), "working");
  assert.equal(tabState(quiet, true, true, complete), "working");
  assert.equal(tabState({ ...quiet, awaiting: "failed", running: true }, true, true, complete), "failed");
  for (const awaiting of ["approval", "input"] as const) {
    assert.equal(tabState({ ...quiet, awaiting, running: true }, true, true, complete), "needsYou");
  }
  assert.equal(tabState({ ...quiet, awaiting: "signIn" }, false, false, complete), "needsYou");
  assert.deepEqual(TAB_STATES, ["needsYou", "failed", "working", "stopped", "done"]);
  assert.deepEqual(Object.values(SESSION_STATUS_WORDS), ["Needs you", "Failed", "Working", "Stopped", "Done"]);
  assert.deepEqual(TAB_STATE_WORDS.done, { one: "Done", many: "done" });
});

test("the tooltip retains question, approval, sign-in and usage-limit reasons", () => {
  for (const [awaiting, reason] of [["approval", "approval"], ["input", "question"], ["signIn", "sign-in"]] as const) {
    assert.deepEqual(sessionStatus({ ...quiet, awaiting }, null), { state: "needsYou", reason });
  }
  const reset = new Date(2026, 9, 18, 14, 5).getTime();
  assert.deepEqual(sessionStatus(quiet, { resetsAtMs: reset }), { state: "working", reason: "waiting for reset 14:05" });
  assert.deepEqual(sessionStatus({ ...quiet, awaiting: "failed" }, { resetsAtMs: reset }), { state: "failed", reason: null });
  assert.deepEqual(sessionStatus(quiet, null, false, true), { state: "done", reason: null });
});

test("git marks still refresh the context card but never choose a row word", () => {
  useSettledTabs.setState({ byTab: { s: { state: "merged" } } });
  assert.equal(sessionStatus(quiet, null), null, "a git merge cannot make a never-run session Done");
  applySettledTabEvents([{ stream: "conversation:s", event: { type: "requestReady", conversationId: "s", requestId: "r", head: "abc" } } as EventEnvelope]);
  assert.deepEqual(useSettledTabs.getState().byTab.s, { state: "ready" });
  assert.equal(sessionStatus(quiet, null), null);
  assert.deepEqual(sessionStatus(quiet, null, false, true), { state: "done", reason: null });
});

test("D61: a session's turn end looks at git again, and only work all on the base leaves nothing to merge", () => {
  // The thread may have pushed its work to the remote's main itself: no merge is recorded, so
  // only a fresh look tells the card. A worktree session never ready is looked at too.
  const idle = { stream: "conversation:s", event: { type: "runStateChanged", conversationId: "s", state: "idle", error: null, requestId: null } } as EventEnvelope;
  useSettledTabs.setState({ byTab: { s: { state: "ready", ahead: 9 } } });
  assert.equal(applySettledTabEvents([idle]), true, "a ready one");
  useSettledTabs.setState({ byTab: {} });
  useApp.setState({ conversations: { s: session({ setup: { type: "session", repo: "/r", environment: { type: "newWorktree", base: "main", branch: "b", path: "/w", start: null }, permission: "full", orchestrator: null, workersSeeUncommitted: null, planMode: null } as unknown as Conversation["setup"] }) } });
  assert.equal(applySettledTabEvents([idle]), true, "a worktree session never ready");
  useApp.setState({ conversations: { s: session({ kind: "chat", projectId: null }) } });
  assert.equal(applySettledTabEvents([idle]), false, "a chat has no branch to look at");
  useApp.setState({ conversations: {} });

  assert.deepEqual(settledByTab([{ conversationId: "r", state: "ready", ahead: 2 }, { conversationId: "m", state: "merged", ahead: 0 }], ["m", "n"]), {
    r: { state: "ready", ahead: 2, landed: false },
    m: { state: "merged", ahead: 0, landed: true },
    n: { landed: true },
  });
  assert.equal(nothingToMerge({ landed: true }), true, "committed and pushed in one turn, never ready");
  assert.equal(nothingToMerge({ state: "merged", ahead: 0, landed: false }), false, "an uncommitted edit since");
  assert.equal(nothingToMerge({ state: "merged" }), true, "just merged, before the fresh look");
  assert.equal(nothingToMerge({ state: "ready", ahead: 9, landed: false }), false);
  assert.equal(nothingToMerge(undefined), false);
});

test("a failed turn needs the user until the next one runs", () => {
  const failed = fromSnapshot({ conversationId: "s", run: "idle", tasks: [], approvals: [], questions: [], failed: true });
  assert.equal(rowActivity(failed).awaiting, "failed");
  assert.equal(rowActivity({ ...failed, turnOpen: true }).awaiting, null);
});

const session = (extra: Partial<Conversation> = {}): Conversation => ({
  id: "s", kind: "session", projectId: "p", title: "s", pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup: null,
  lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null, ...extra,
});

test("a project's open session is a task of its own; a chat, a side chat, an archived one aren't", () => {
  assert.equal(isTaskTab(session()), true);
  assert.equal(isTaskTab(session({ projectId: null })), false);
  assert.equal(isTaskTab(session({ kind: "chat" })), false);
  assert.equal(isTaskTab(session({ sideOf: "other" })), false);
  assert.equal(isTaskTab(session({ lifecycle: "archived" })), false);
});


test("Needs you keeps a blue dot, failures a red mark, Working a spinner, and Done only an unread dot", () => {
  for (const awaiting of ["approval", "input", "signIn"] as const) for (const running of awaiting === "signIn" ? [false] : [false, true]) for (const unread of [false, true]) {
    assert.deepEqual(sidebarIndicators({ ...quiet, awaiting, running }, true, unread, false, true), { working: false, error: false, unread: true });
  }
  assert.deepEqual(sidebarIndicators({ ...quiet, awaiting: "failed", running: true }, true, true), { working: false, error: true, unread: false });
  assert.deepEqual(sidebarIndicators({ ...quiet, running: true }, false, true), { working: true, error: false, unread: false });
  assert.deepEqual(sidebarIndicators(quiet, false, true, true, true), { working: false, error: false, unread: false });
  assert.deepEqual(sidebarIndicators(quiet, false, true, false, true), { working: false, error: false, unread: true });
  assert.deepEqual(sidebarIndicators(quiet, false, false, false, true), { working: false, error: false, unread: false });
  assert.deepEqual(sidebarIndicators(quiet, false, true), { working: false, error: false, unread: false });
});

test("a session the user is looking at, or working again past a sign-in, doesn't show Needs you", () => {
  // The screenshot's session: an MCP server still wants a sign-in, and the user's reply set it working.
  const signedOff = { ...quiet, awaiting: "signIn", running: true, workers: 1 } as const;
  assert.equal(tabState(signedOff, false), "working");
  assert.deepEqual(sessionStatus(signedOff, null), { state: "working", reason: null });
  assert.deepEqual(sidebarIndicators(signedOff, false, false), { working: true, error: false, unread: false });
  // In front of the user, no waiting card marks its own row or tab; the next state shows instead.
  for (const awaiting of ["approval", "input", "signIn"] as const) {
    assert.equal(tabState({ ...quiet, awaiting, running: true }, false, false, false, true), "working");
    assert.equal(tabState({ ...quiet, awaiting }, false, false, true, true), "done");
    assert.deepEqual(sessionStatus({ ...quiet, awaiting }, null, false, false, true), null);
    assert.deepEqual(sidebarIndicators({ ...quiet, awaiting }, false, false, false, true, true), { working: false, error: false, unread: false });
  }
  // Elsewhere, a question still needs the user.
  assert.deepEqual(sidebarIndicators({ ...quiet, awaiting: "input", running: true }, false, false), { working: false, error: false, unread: true });
});
