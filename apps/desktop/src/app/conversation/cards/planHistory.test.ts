import assert from "node:assert/strict";
import { test } from "node:test";

import { earlierPlans } from "@/app/conversation/cards/planHistory";
import type { Plan } from "@/ipc/generated";

const plan = (id: string, revises: string | null = null): Plan => ({ id, revises }) as unknown as Plan;
const ids = (list: Plan[]) => list.map((p) => p.id);

test("earlier plans keep the revisions each of them replaced, newest first", () => {
  // a1 was revised into a2; b1 into b2; c is current, revising nothing.
  const plans = Object.fromEntries(
    [plan("a1"), plan("a2", "a1"), plan("b1"), plan("b2", "b1"), plan("c")].map((p) => [p.id, p]),
  );
  assert.deepEqual(ids(earlierPlans(plans, plans.c!, ["a2", "b2"])), ["b2", "b1", "a2", "a1"]);
  // The current plan's own revisions come first, and nothing shows twice.
  const current = plan("b3", "b2");
  assert.deepEqual(ids(earlierPlans({ ...plans, b3: current }, current, ["a2", "b2"])), ["b2", "b1", "a2", "a1"]);
});
