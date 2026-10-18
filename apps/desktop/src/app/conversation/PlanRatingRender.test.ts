import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("a plan is rated Good plan or Bad plan, and a plan's or an answer's rating can be cleared (issue 127)", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "plan-rating.html", "plan-rating-result", 90000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  assert.equal(seen.planLabel, "Rate this plan");
  // Clear rating only once there is a rating to clear.
  assert.deepEqual(seen.planMenu, ["Good plan", "Bad plan"]);
  assert.deepEqual(seen.ratedMenu, ["Good plan", "Bad plan", "Clear rating"]);
  assert.deepEqual(seen.answerMenu, ["Good response", "Bad response", "Clear rating"]);
  assert.deepEqual(seen.sent, ["plan:p1 good", "plan:p1 cleared", "a1 cleared"]);
});
