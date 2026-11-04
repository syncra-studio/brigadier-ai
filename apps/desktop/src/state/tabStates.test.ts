import assert from "node:assert/strict";
import { test } from "node:test";

import type { Conversation, EventEnvelope } from "@/ipc/generated";
import { fromSnapshot, rowActivity } from "@/state/activity";
import { useApp } from "@/state/store";
import { TAB_STATES, TAB_STATE_WORDS, applySettledTabEvents, isTaskTab, rowMark, settledByTab, tabState, useSettledTabs } from "@/state/tabStates";

const quiet = { running: false, awaiting: null, workers: 0 } as const;

test("session states use only activity, in the requested precedence order", () => {
  const complete = true;
  assert.equal(tabState(quiet, false), null);
  assert.deepEqual(rowMark(quiet), { look: null, hover: null }, "no Done mark on a finished, read row or tab");
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
  assert.equal("done" in TAB_STATE_WORDS, false, "a finished session has no state word");
});

test("rowMark: the four looks of every row state, with their hover (§2)", () => {
  const reset = new Date(2026, 9, 18, 14, 5).getTime();
  const working = { look: "working", hover: "Working" } as const;
  const none = { look: null, hover: null } as const;
  const cases: [string, Parameters<typeof rowMark>[0], Parameters<typeof rowMark>[1], ReturnType<typeof rowMark>][] = [
    ["CLI turn open, run starting/running, a worker or a held command", { ...quiet, running: true }, {}, working],
    ["waiting on a usage limit", quiet, { quotaWait: { resetsAtMs: reset } }, { look: "working", hover: "Waiting for usage limit · resets 14:05" }],
    ["waiting on a usage limit, reset unknown", quiet, { quotaWait: { resetsAtMs: null } }, { look: "working", hover: "Waiting for usage limit" }],
    ["working past a usage limit", { ...quiet, running: true }, { quotaWait: { resetsAtMs: reset } }, working],
    ["an approval", { ...quiet, awaiting: "approval", running: true }, { unread: true }, { look: "dot", amber: true, hover: "Needs you · approval" }],
    ["a question", { ...quiet, awaiting: "input" }, {}, { look: "dot", amber: true, hover: "Needs you · question" }],
    ["an MCP sign-in", { ...quiet, awaiting: "signIn" }, {}, { look: "dot", amber: true, hover: "Needs you · sign-in" }],
    ["a sign-in while it works on", { ...quiet, awaiting: "signIn", running: true }, {}, working],
    ["failed or cut off", { ...quiet, awaiting: "failed" }, { unread: true, quotaWait: { resetsAtMs: reset } }, { look: "error", amber: true, hover: "Failed" }],
    ["finished, not opened since", quiet, { unread: true }, { look: "dot", hover: "New reply" }],
    ["finished and read", quiet, {}, none],
    ["stopped", quiet, { stopped: true, unread: true }, none],
    ["idle, never ran", quiet, {}, none],
    ["needs you, in front of the user", { ...quiet, awaiting: "approval" }, { viewing: true }, none],
  ];
  for (const [what, activity, options, mark] of cases) assert.deepEqual(rowMark(activity, options), mark, what);
});

test("rowMark: a sleeping row keeps an unread dot it had, and shows nothing once read (D12)", () => {
  const asleep = fromSnapshot({ conversationId: "s", run: "hibernated", tasks: [], approvals: [], questions: [], turnEndedAtMs: 5 });
  assert.deepEqual(rowMark(rowActivity(asleep), { unread: true }), { look: "dot", hover: "New reply" });
  assert.deepEqual(rowMark(rowActivity(asleep), { unread: false }), { look: null, hover: null });
  // Asleep, nothing spins and no old failure shows (D10).
  assert.equal(rowActivity({ ...asleep, turnOpen: true, workersWorking: true }).running, false);
  assert.equal(rowActivity({ ...asleep, ended: "failed" }).awaiting, null);
});

test("git marks still refresh the context card but never choose a row mark", () => {
  useSettledTabs.setState({ byTab: { s: { state: "merged" } } });
  assert.deepEqual(rowMark(quiet), { look: null, hover: null }, "a git merge cannot mark a never-run session");
  applySettledTabEvents([{ stream: "conversation:s", event: { type: "requestReady", conversationId: "s", requestId: "r", head: "abc" } } as EventEnvelope]);
  assert.deepEqual(useSettledTabs.getState().byTab.s, { state: "ready" });
  assert.deepEqual(rowMark(quiet), { look: null, hover: null });
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


test("a session the user is looking at, or working again past a sign-in, doesn't show Needs you", () => {
  // The screenshot's session: an MCP server still wants a sign-in, and the user's reply set it working.
  const signedOff = { ...quiet, awaiting: "signIn", running: true, workers: 1 } as const;
  assert.equal(tabState(signedOff, false), "working");
  // In front of the user, no waiting card marks its own row or tab; the next state shows instead.
  for (const awaiting of ["approval", "input", "signIn"] as const) {
    assert.equal(tabState({ ...quiet, awaiting, running: true }, false, false, false, true), "working");
    assert.equal(tabState({ ...quiet, awaiting }, false, false, true, true), "done");
    assert.deepEqual(rowMark({ ...quiet, awaiting }, { viewing: true }), { look: null, hover: null });
  }
});
