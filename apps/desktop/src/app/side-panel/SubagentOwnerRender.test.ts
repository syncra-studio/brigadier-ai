import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// #760: a worker's subagent is found in that worker's thread, which its panel keeps and reads.
test("a worker's subagent panel keeps its worker's thread through a resync, and reads it when it isn't held", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "subagent-owner.html", "subagent-owner-result", 10000)) as Record<string, unknown>;
  assert.deepEqual(seen, { keptThroughResync: true, readWhenMissing: true });
});
