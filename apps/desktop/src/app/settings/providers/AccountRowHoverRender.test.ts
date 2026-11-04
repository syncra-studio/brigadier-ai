import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// Under the pointer an account is one lit block, header and limits together, and the lines above
// and below it hide without moving anything; the lines come back when the pointer leaves.
test("a hovered account lights as one block and hides the lines touching it", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "providers.html?drive=hover", "providers-result", 30000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<string, unknown> & { wash: string; clear: string };
  const lit = (lines: string[], parts: string[]) => ({ fill: seen.wash, parts, fillsCard: true, lines, moved: false });

  // The card's children in order: the switch's row, three accounts, Add another.
  assert.deepEqual(seen.rest, ["1", "1", "1", "1", "1"]);
  assert.deepEqual(seen.expandedHeader, lit(["0", "0", "1", "1", "1"], [seen.clear, seen.clear]));
  assert.deepEqual(seen.expandedLimits, lit(["0", "0", "1", "1", "1"], [seen.clear, seen.clear]));
  assert.deepEqual(seen.collapsed, lit(["1", "0", "0", "1", "1"], [seen.clear]));
  assert.deepEqual(seen.last, lit(["1", "1", "0", "0", "1"], [seen.clear]));
  assert.deepEqual(seen.left, ["1", "1", "1", "1", "1"]);
});
