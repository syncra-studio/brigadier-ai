import assert from "node:assert/strict";
import { test } from "node:test";

import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import { pendingActionKeys } from "@/app/conversation/pendingActions";
import type { Conversation, OvernightRun, Plan, Question, Setup } from "@/ipc/generated";
import type { ShownApproval } from "@/state/board";

type Session = Extract<Setup, { type: "session" }>;
const setup: Session = {
  type: "session", repo: "/fixture", environment: { type: "localCheckout", branch: "main" },
  permission: "fullAccess", planMode: false, workersSeeUncommitted: null,
  orchestrator: { provider: "claude", model: null, effort: null },
};
const conversation: Pick<Conversation, "id" | "setup"> = { id: night.conversationId, setup };
const plan: Plan = {
  ...Object.values(night.plans as unknown as Record<string, Plan>)[0]!,
  id: "plan", position: 2, state: { type: "proposed" },
};
const run: OvernightRun = {
  ...Object.values(night.overnight as unknown as Record<string, OvernightRun>)[0]!,
  id: "night", state: "proposed",
};
const approval: ShownApproval = {
  id: "approval", conversationId: conversation.id, taskId: null, requestId: null,
  position: 3, subject: { type: "action", action: "Continue", details: "" },
  state: { type: "pending" }, createdAtMs: 0, resolvedAtMs: null,
};
const question: Question = {
  id: "question", conversationId: conversation.id, taskId: null, requestId: null,
  position: 1, kind: { type: "orchestrator" }, text: "Which branch?", options: [], recommended: null,
  items: [], answer: null, answers: [], createdAtMs: 0, answeredAtMs: null,
};
const board = {
  conversationId: conversation.id, approvals: { approval }, questions: { question },
  plans: { plan }, overnight: { night: run },
};

test("overnight proposals stay reachable after older questions and approvals under Full access", () => {
  assert.deepEqual(pendingActionKeys(conversation, board), ["question:question", "approval:approval", "overnight:night"]);
});

test("approval and plan modes include proposed plans in their original order", () => {
  for (const change of [{ permission: "askForApproval" as const }, { planMode: true }]) {
    assert.deepEqual(pendingActionKeys({ ...conversation, setup: { ...setup, ...change } }, board),
      ["question:question", "plan:plan", "approval:approval", "overnight:night"]);
  }
});

test("settled decisions and runs leave the rail", () => {
  assert.deepEqual(pendingActionKeys(conversation, {
    ...board,
    approvals: { approval: { ...approval, state: { type: "allowed", by: "user", similar: false } } },
    questions: { question: { ...question, answer: "main", answeredAtMs: 1 } },
    overnight: { night: { ...run, state: "running" } },
  }), []);
  assert.deepEqual(pendingActionKeys(conversation, {
    ...board, approvals: {}, overnight: {},
    questions: { question: { ...question, answeredAtMs: 1 } },
  }), []);
});

test("cards from another conversation or an unloaded board do not appear", () => {
  assert.deepEqual(pendingActionKeys(null, board), []);
  assert.deepEqual(pendingActionKeys(conversation, null), []);
  assert.deepEqual(pendingActionKeys({ ...conversation, id: "other" }, board), []);
});
