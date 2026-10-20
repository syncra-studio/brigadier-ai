import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("the timeline lists events in order, marks the one at work, and joins them by a line", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "timeline.html", "timeline-result", 30000));
  assert.deepEqual(seen.titles, ["Merged 3 commits into main", "Merged 1 commit into main", "Reviewing 2 commits", "Merge into main"]);
  assert.deepEqual(seen.current, [null, null, "step", null]);
  assert.deepEqual(seen.times, ["02:14", "03:40", "04:05", ""]);
  assert.deepEqual(seen.lines, [2, 2, 2, 1]);
  // Without a time on any event there is no time column: the dot and the text only.
  assert.equal(seen.untimedColumns, 2);
});
