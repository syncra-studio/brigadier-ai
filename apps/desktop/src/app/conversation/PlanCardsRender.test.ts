import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage } from "@/fixtures/headless";

test("the plan cards page draws its plans, the context card's among them", { timeout: 60000 }, async (t) => {
  const drawn = JSON.parse(await renderFixturePage(t, "plan-cards.html", "plan-cards-result", 5000)) as string[];
  assert.ok(drawn.length > 1, `plan sections drawn: ${JSON.stringify(drawn)}`);
  assert.ok(drawn.includes("plan-normal"), `the context card's plan: ${JSON.stringify(drawn)}`);
});
