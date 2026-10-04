import assert from "node:assert/strict";
import { test } from "node:test";

import { earlierPlans } from "@/app/conversation/cards/planHistory";
import type { Plan } from "@/ipc/generated";

const plan = (id: string): Plan => ({ id }) as unknown as Plan;
const ids = (list: Plan[]) => list.map((p) => p.id);

test("earlier plans list the plans before the current one, newest first", () => {
  const plans = Object.fromEntries([plan("a"), plan("b"), plan("c")].map((p) => [p.id, p]));
  assert.deepEqual(ids(earlierPlans(plans, plans.c!, ["a", "b"])), ["b", "a"]);
  // The current plan never shows as an earlier one, and nothing shows twice.
  assert.deepEqual(ids(earlierPlans(plans, plans.c!, ["a", "b", "c", "b"])), ["b", "a"]);
});
