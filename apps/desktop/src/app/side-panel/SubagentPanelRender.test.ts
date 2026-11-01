import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

test("a running subagent's panel follows its transcript as its file grows, and shows its closing words once it is over; one over before its panel opened shows its transcript", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "subagent-panel.html", "subagent-panel-result", 10000)) as Record<string, unknown>;
  assert.deepEqual(seen, { whileWorking: true, closing: true, overBeforeOpen: true });
});
