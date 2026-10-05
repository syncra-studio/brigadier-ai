import assert from "node:assert/strict";
import { test } from "node:test";

import { activePlanRequest, capsuleMode, contextPlanId, currentRequestPlan, planProgress, planStepStatus } from "@/app/conversation/planProgress";
import type { Plan, PlanState, TaskState, UserRequest } from "@/ipc/generated";

function plan(state: PlanState, id = "plan", createdAtMs = 1): Plan {
  return {
    id, conversationId: "session", requestId: "request", position: createdAtMs,
    title: "Plan", steps: ["First", "Second", "Third"].map((title, index) => ({ title, detail: null, taskId: `task-${index}`, stage: "pending" as const, startedAtMs: null, endedAtMs: null, outline: null })),
    state, createdAtMs, decidedAtMs: null,
  };
}
function request(state: UserRequest["state"], id = "request", startedAtMs = 1): UserRequest {
  return { id, conversationId: "session", preview: "Request", state, startedAtMs, endedAtMs: null, steeredInto: null, steeredAfter: null, undo: null };
}
const approved = plan({ type: "approved", by: "orchestrator" });
const label = (states: (TaskState | undefined)[]) => planProgress(approved, states).label;

test("each plan lifecycle takes precedence over its steps", () => {
  const cases: [PlanState, string, string][] = [
    [{ type: "proposed" }, "Plan proposed", "review"],
    [{ type: "approved", by: "user" }, "Approved, not started", "pending"],
    [{ type: "rejected", message: null }, "Plan rejected", "failed"],
    [{ type: "superseded" }, "Plan superseded", "pending"],
  ];
  for (const [state, expected, status] of cases) {
    assert.equal(planProgress(plan(state), []).label, expected);
    assert.equal(planProgress(plan(state), []).status, status);
    if (state.type !== "approved") assert.equal(planProgress(plan(state), ["running", "failed", "done"]).label, expected);
  }
});

test("task states have distinct pending, running, landed and failed marks", () => {
  assert.equal(planStepStatus(undefined), "pending");
  assert.equal(planStepStatus("queued"), "pending");
  for (const state of ["starting", "running", "blocked", "paused", "reported", "landing", "readyToLand"] as const) assert.equal(planStepStatus(state), "active");
  for (const state of ["landed", "done"] as const) assert.equal(planStepStatus(state), "done");
  for (const state of ["failed", "rejected", "stopped"] as const) assert.equal(planStepStatus(state), "failed");
});

test("progress names the real running index, simultaneous workers and failures", () => {
  assert.equal(label(["queued", undefined, "queued"]), "Approved, not started");
  assert.equal(label(["queued", "running", undefined]), "Step 2 of 3: Second");
  assert.equal(label(["landed", "running", "starting"]), "Step 2 of 3: Second · 2 running");
  assert.equal(label(["landed", "failed", "running"]), "Step 2 failed");
  assert.equal(label(["done", "rejected", undefined]), "Step 2 failed");
  assert.equal(label(["done", "stopped", undefined]), "Step 2 failed");
  assert.equal(label(["landed", "queued", undefined]), "1 of 3 done");
  assert.equal(label(["landed", "done", "landed"]), "3 of 3 done");
  assert.equal(planProgress(approved, ["landed", "done", "landed"]).status, "done");
  assert.equal(planProgress({ ...approved, steps: [] }, []).label, "Approved, not started");
});

test("active request transitions preserve all plan lifecycles until the request ends", () => {
  const working = request({ type: "working" });
  for (const state of [{ type: "proposed" }, { type: "approved", by: "orchestrator" }, { type: "rejected", message: null }] as const) {
    const current = plan(state);
    const active = activePlanRequest({ request: working });
    assert.equal(currentRequestPlan({ plan: current }, active?.id ?? null), current);
    assert.equal(capsuleMode(false, true, active, current), "request");
  }
  const waiting = request({ type: "waiting" });
  for (const state of [{ type: "proposed" }] as const) {
    const current = plan(state);
    assert.equal(activePlanRequest({ request: waiting }), waiting);
    assert.equal(capsuleMode(false, true, waiting, current), "request");
  }
  for (const state of [{ type: "done" }, { type: "stopped" }, { type: "failed", error: "failure" }] as const) {
    const active = activePlanRequest({ request: request(state) });
    assert.equal(currentRequestPlan({ approved }, active?.id ?? null), null);
    assert.equal(active, null);
    assert.equal(capsuleMode(false, false, active, null), null);
    assert.equal(capsuleMode(false, false, request(state), approved), null);
  }
});

test("a newer plan replaces an approved one immediately, including tied timestamps", () => {
  const revision = plan({ type: "proposed" }, "revision", 2);
  assert.equal(currentRequestPlan({ approved, revision }, "request"), revision);
  const tied = { ...revision, createdAtMs: approved.createdAtMs };
  assert.equal(currentRequestPlan({ approved, tied }, "request"), tied);
  assert.equal(currentRequestPlan({ predecessor: { ...approved, state: { type: "superseded" } }, revision }, "request"), revision);
  assert.equal(currentRequestPlan({ superseded: plan({ type: "superseded" }) }, "request"), null);
  const rejected = { ...revision, state: { type: "rejected", message: null } } as Plan;
  assert.equal(currentRequestPlan({ approved, rejected }, "request"), rejected);
});

test("a newer request without a plan never inherits an older request's plan", () => {
  const older = request({ type: "working" });
  const newer = request({ type: "waiting" }, "newer", 2);
  assert.equal(activePlanRequest({ older, newer }), newer);
  assert.equal(currentRequestPlan({ approved }, newer.id), null);
  assert.equal(capsuleMode(false, false, newer, null), null);
  for (const state of [{ type: "done" }, { type: "stopped" }, { type: "failed", error: "failure" }] as const) {
    assert.equal(activePlanRequest({ older, newer: { ...newer, state } }), older);
    const waiting = { ...older, state: { type: "waiting" } } as UserRequest;
    assert.equal(activePlanRequest({ older: waiting, newer: { ...newer, state } }), waiting);
  }
  assert.equal(activePlanRequest({ older, steered: { ...newer, steeredInto: older.id } }), older);
});

test("done and failed execution remain visible while active; no-plan and overnight behavior stays", () => {
  const active = request({ type: "working" });
  for (const states of [["landed", "landed", "done"], ["done", "failed", "queued"]] as TaskState[][]) {
    assert.match(planProgress(approved, states).label, /done|failed/);
    assert.equal(capsuleMode(false, false, active, approved), "request");
  }
  assert.equal(capsuleMode(false, false, active, null), "request");
  assert.equal(capsuleMode(false, true, active, null), null);
  assert.equal(capsuleMode(true, false, null, approved), "run");
  assert.equal(capsuleMode(true, true, active, approved), null);
  assert.equal(capsuleMode(false, false, request({ type: "working" }, "run-phase"), approved), null);
});

test("the context card keeps the latest session plan after requests end, without leaking it into a newer active request", () => {
  const older = approved;
  const latest = plan({ type: "rejected", message: "Change the approach" }, "latest", 2);
  const plans = { older, latest };
  const ids = [older.id, latest.id];
  for (const state of [{ type: "done" }, { type: "stopped" }, { type: "failed", error: "failure" }] as const) {
    const active = activePlanRequest({ request: request(state) });
    assert.equal(contextPlanId(plans, ids, active), latest.id);
    assert.equal(capsuleMode(false, false, active, latest), null);
  }
  assert.equal(contextPlanId(plans, ids, null), latest.id);
  assert.equal(contextPlanId(plans, ids, request({ type: "working" })), latest.id);
  for (const state of [{ type: "working" }, { type: "waiting" }] as const) {
    assert.equal(contextPlanId(plans, ids, request(state, "newer", 3)), null);
  }
  assert.equal(contextPlanId(plans, [], null), null);
  assert.equal(contextPlanId(plans, [older.id], request({ type: "working" })), null);
});
