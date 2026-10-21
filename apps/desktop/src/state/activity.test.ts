import assert from "node:assert/strict";
import { test } from "node:test";

import type { EventEnvelope, Plan, PlanState } from "@/ipc/generated";
import { applyActivityEvents, fromSnapshot, rowActivity, useActivity } from "@/state/activity";
import { sessionStatus } from "@/state/tabStates";

const plan = (state: PlanState): EventEnvelope => ({
  stream: "conversation:c1",
  event: {
    type: "planUpdated",
    plan: {
      id: "p1", conversationId: "c1", requestId: "r1", position: 1, title: "Add a --version flag", body: "# Add a --version flag",
      steps: [], state, createdAtMs: 0, decidedAtMs: null,
    } as Plan,
  },
} as EventEnvelope);

test("a proposed plan has the sidebar say Awaiting approval, and Not now ends it", () => {
  applyActivityEvents([plan({ type: "proposed" })]);
  assert.equal(rowActivity(useActivity.getState().byConversation.c1).awaiting, "approval");
  applyActivityEvents([plan({ type: "putOff" })]);
  assert.equal(rowActivity(useActivity.getState().byConversation.c1).awaiting, null);
});

const on = (conversationId: string, event: unknown): EventEnvelope =>
  ({ stream: `conversation:${conversationId}`, event }) as EventEnvelope;

const phaseOf = (conversationId: string) =>
  rowActivity(useActivity.getState().byConversation[conversationId]).phase;

const stepOn = (conversationId: string, kind: object): EventEnvelope =>
  on(conversationId, { type: "orchestratorStepped", step: { requestId: "r1", atMs: 0, position: 0, kind } });

const ciWatch = (result?: string) =>
  on("c2", {
    type: "orchestratorStepped",
    step: {
      requestId: "r1", atMs: 0, position: 0,
      kind: { type: "ciWatch", repo: "o/n", branch: "main", sha: "abc", waiting: false, runs: [], result },
    },
  });

test("a CI watch has an idle session's row say Watching CI until it ends", () => {
  applyActivityEvents([ciWatch()]);
  assert.deepEqual(rowActivity(useActivity.getState().byConversation.c2), {
    running: false, awaiting: null, phase: "watchingCi", workers: 0,
  });
  applyActivityEvents([ciWatch("passed")]);
  assert.equal(rowActivity(useActivity.getState().byConversation.c2).phase, null);
});

test("a review outranks checks, and the thread's run_check ends with its turn", () => {
  const tool = (status: string) =>
    on("c3", {
      type: "orchestratorStepped",
      step: {
        requestId: "r1", atMs: 0, position: 0,
        kind: { type: "tool", itemId: "i1", name: "mcp__brigadier__run_check", detail: null, status, throughPosition: 0 },
      },
    });
  const review = (type: string) =>
    on("c3", { type: "reviewUpdated", review: { id: "rv1", conversationId: "c3", state: { type } } });
  applyActivityEvents([on("c3", { type: "runStateChanged", conversationId: "c3", state: "running" }), tool("inProgress")]);
  assert.equal(phaseOf("c3"), "checking");
  applyActivityEvents([review("running")]);
  assert.equal(phaseOf("c3"), "reviewing");
  applyActivityEvents([review("clean")]);
  assert.equal(phaseOf("c3"), "checking");
  applyActivityEvents([on("c3", { type: "runStateChanged", conversationId: "c3", state: "idle" })]);
  assert.equal(phaseOf("c3"), null);
});

test("a held merge says Merging until it merges", () => {
  applyActivityEvents([stepOn("c4", { type: "mergeHeld", askedIn: "m1" })]);
  assert.equal(phaseOf("c4"), "merging");
  applyActivityEvents([stepOn("c4", { type: "merged", branch: "b", base: "main", commits: 1, askedIn: "m1" })]);
  assert.equal(phaseOf("c4"), null);
  // A fresh consent replaces the held one: cancelling it leaves nothing held.
  applyActivityEvents([stepOn("c4", { type: "mergeHeld", askedIn: "m2" }), stepOn("c4", { type: "mergeHeld", askedIn: "m3" })]);
  applyActivityEvents([stepOn("c4", { type: "mergeCancelled", askedIn: "m3" })]);
  assert.equal(phaseOf("c4"), null);
});


test("a stopped request is named through reconnect and clears when another turn starts", () => {
  const snapshot = fromSnapshot({ conversationId: "stopped", run: "idle", tasks: [], approvals: [], questions: [], phases: [], stopped: true });
  useActivity.setState({ byConversation: { stopped: snapshot } });
  assert.equal(useActivity.getState().byConversation.stopped?.stopped, true);
  applyActivityEvents([on("stopped", { type: "runStateChanged", conversationId: "stopped", state: "starting" })]);
  assert.equal(useActivity.getState().byConversation.stopped?.stopped, false);
  applyActivityEvents([on("stopped", { type: "requestUpdated", request: { conversationId: "stopped", state: { type: "stopped" } } })]);
  assert.equal(useActivity.getState().byConversation.stopped?.stopped, true);
  // An earlier request's update (its review kept) leaves the stop be; the next turn clears it.
  applyActivityEvents([on("stopped", { type: "requestUpdated", request: { conversationId: "stopped", state: { type: "done" } } })]);
  assert.equal(useActivity.getState().byConversation.stopped?.stopped, true);
  applyActivityEvents([on("stopped", { type: "requestUpdated", request: { conversationId: "stopped", state: { type: "working" } } })]);
  assert.equal(useActivity.getState().byConversation.stopped?.stopped, false);
});

test("a thread that fails while its worker runs reads Needs you · failed, not Working (issue 363)", () => {
  applyActivityEvents([
    on("c5", { type: "runStateChanged", conversationId: "c5", state: "running" }),
    on("c5", { type: "taskUpdated", task: { id: "t1", conversationId: "c5", state: "running", kind: "write", title: "Add a test" } }),
    on("c5", { type: "runStateChanged", conversationId: "c5", state: "failed" }),
  ]);
  const row = rowActivity(useActivity.getState().byConversation.c5);
  assert.equal(row.running, true);
  assert.equal(row.awaiting, "failed");
  assert.deepEqual(sessionStatus(row, null, undefined), { state: "failed", reason: null });
});

test("a request working between turns reads Working, not Ready, as its composer does (issue 485)", () => {
  const snapshot = fromSnapshot({ conversationId: "between", run: "idle", tasks: [], approvals: [], questions: [], phases: [], requests: ["r1"] });
  useActivity.setState({ byConversation: { between: snapshot } });
  const working = rowActivity(useActivity.getState().byConversation.between);
  assert.equal(working.running, true);
  assert.deepEqual(sessionStatus(working, null, { state: "ready" }), { state: "working", reason: null });
  applyActivityEvents([on("between", { type: "requestUpdated", request: { id: "r1", conversationId: "between", state: { type: "done" } } })]);
  assert.deepEqual(sessionStatus(rowActivity(useActivity.getState().byConversation.between), null, { state: "ready" }), { state: "ready", reason: null });
  applyActivityEvents([on("between", { type: "requestUpdated", request: { id: "r2", conversationId: "between", state: { type: "working" } } })]);
  assert.equal(rowActivity(useActivity.getState().byConversation.between).running, true);
});
