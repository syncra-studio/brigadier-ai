import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage } from "@/fixtures/headless";

test("the plan cards page draws its plans' rows, and no context card holds a plan section (Q88)", { timeout: 60000 }, async (t) => {
  const drawn = JSON.parse(await renderFixturePage(t, "plan-cards.html", "plan-cards-result", 5000)) as { rows: string[]; inContext: number };
  assert.ok(drawn.rows.length > 1, `plan rows drawn: ${JSON.stringify(drawn)}`);
  assert.equal(drawn.inContext, 0);
});
