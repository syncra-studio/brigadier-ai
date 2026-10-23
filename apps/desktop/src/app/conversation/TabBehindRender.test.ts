import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// A Browser tab put behind its session's conversation (#417): its toolbar's buttons hide with
// it at once, none drawn over the thread while a transition of theirs would run.
test("a tab put behind hides every button of its toolbar at once", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "tab-behind.html?drive=1", "tab-behind-result", 60000)) as {
    front: number;
    behind: string[];
    skipped: string | null;
  };
  assert.ok(seen.front >= 4, `the toolbar's buttons show in front (${seen.front})`);
  assert.deepEqual(seen.behind, []);
  // #569: skipped, so a page behind holds no layers.
  assert.equal(seen.skipped, "hidden");
});
