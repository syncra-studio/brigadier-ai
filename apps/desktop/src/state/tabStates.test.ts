import assert from "node:assert/strict";
import { test } from "node:test";

import type { Conversation, EventEnvelope } from "@/ipc/generated";
import { fromSnapshot, rowActivity } from "@/state/activity";
import { TAB_STATES, TAB_STATE_WORDS, SESSION_STATUS_WORDS, applySettledTabEvents, isTaskTab, nothingToMerge, sessionStatus, sidebarIndicators, tabState, useSettledTabs } from "@/state/tabStates";

const quiet = { running: false, awaiting: null, workers: 0 } as const;

test("session states use only activity, in the requested precedence order", () => {
  const complete = true;
  assert.equal(tabState(quiet, false), null);
  assert.equal(tabState(quiet, false, false, complete), "done");
  assert.equal(tabState(quiet, false, true, complete), "stopped");
  assert.equal(tabState({ ...quiet, running: true }, false, true, complete), "working");
  assert.equal(tabState(quiet, true, true, complete), "working");
  assert.equal(tabState({ ...quiet, awaiting: "failed", running: true }, true, true, complete), "failed");
  for (const awaiting of ["approval", "input", "signIn"] as const) {
    assert.equal(tabState({ ...quiet, awaiting, running: true }, true, true, complete), "needsYou");
  }
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

test("D61: a ready session's turn end looks at git again, and merged leaves nothing to merge", () => {
  // The thread pushed its work to the remote's main itself: no merge is recorded, so only a
  // fresh look tells the card it is no longer ahead.
  useSettledTabs.setState({ byTab: { s: { state: "ready", ahead: 9 } } });
  const idle = { stream: "conversation:s", event: { type: "runStateChanged", conversationId: "s", state: "idle", error: null, requestId: null } } as EventEnvelope;
  assert.equal(applySettledTabEvents([idle]), true, "looked at again");
  useSettledTabs.setState({ byTab: {} });
  assert.equal(applySettledTabEvents([idle]), false, "a session never ready or merged isn't");
  assert.equal(nothingToMerge({ state: "ready", ahead: 9 }), false);
  assert.equal(nothingToMerge({ state: "merged", ahead: 0 }), true);
  assert.equal(nothingToMerge({ state: "merged" }), true, "just merged, before the fresh count");
  assert.equal(nothingToMerge({ state: "reverted", ahead: 0 }), false);
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
  for (const awaiting of ["approval", "input", "signIn"] as const) for (const running of [false, true]) for (const unread of [false, true]) {
    assert.deepEqual(sidebarIndicators({ ...quiet, awaiting, running }, true, unread, false, true), { working: false, error: false, unread: true });
  }
  assert.deepEqual(sidebarIndicators({ ...quiet, awaiting: "failed", running: true }, true, true), { working: false, error: true, unread: false });
  assert.deepEqual(sidebarIndicators({ ...quiet, running: true }, false, true), { working: true, error: false, unread: false });
  assert.deepEqual(sidebarIndicators(quiet, false, true, true, true), { working: false, error: false, unread: false });
  assert.deepEqual(sidebarIndicators(quiet, false, true, false, true), { working: false, error: false, unread: true });
  assert.deepEqual(sidebarIndicators(quiet, false, false, false, true), { working: false, error: false, unread: false });
  assert.deepEqual(sidebarIndicators(quiet, false, true), { working: false, error: false, unread: false });
});
