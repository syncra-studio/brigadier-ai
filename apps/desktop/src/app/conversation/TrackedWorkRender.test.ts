import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("summary terminal filtering, Done, history opening and forget use the real controls", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "tracked-work.html", "tracked-work-result", 60000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined);
  assert.equal(seen.initialRows, 1);
  assert.equal(seen.hasOverdue, true);
  assert.equal(seen.historyPanel, "panel:pr-history");
  assert.equal(seen.waitGone, true);
  assert.equal(seen.rowsAfterForget, 0);
  assert.equal(seen.historyRetained, true);
});
