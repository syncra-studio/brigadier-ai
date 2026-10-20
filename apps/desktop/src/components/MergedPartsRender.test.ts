import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// Each group of look-alike parts was merged into one component; these read that one look back.
test("merged parts draw one look per group", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "merged-parts.html", "merged-parts-result", 30000));

  // Labelled divider: words between two 1px hairlines in the border colour, 8px above and below.
  assert.equal(seen.divider.dividers.length, 3);
  for (const divider of seen.divider.dividers) {
    assert.equal(divider.paddingTop, 8);
    assert.equal(divider.color, seen.divider.muted);
    assert.equal(divider.fontSize, seen.divider.textSm);
    assert.deepEqual(divider.rules.map((rule: { height: number }) => rule.height), [1, 1]);
    for (const rule of divider.rules) assert.equal(rule.background, seen.divider.border);
  }
});
