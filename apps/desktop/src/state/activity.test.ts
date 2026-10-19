import assert from "node:assert/strict";
import { test } from "node:test";

import type { EventEnvelope, Plan, PlanState } from "@/ipc/generated";
import { applyActivityEvents, fromSnapshot, rowActivity, useActivity } from "@/state/activity";

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
  applyActivityEvents([on("stopped", { type: "requestUpdated", request: { conversationId: "stopped", state: { type: "done" } } })]);
  assert.equal(useActivity.getState().byConversation.stopped?.stopped, false);
});
