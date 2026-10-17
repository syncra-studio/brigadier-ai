import assert from "node:assert/strict";
import { test } from "node:test";

import type { EventEnvelope, Plan, PlanState } from "@/ipc/generated";
import { applyActivityEvents, rowActivity, useActivity } from "@/state/activity";

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
