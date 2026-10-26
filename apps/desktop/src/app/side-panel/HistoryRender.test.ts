import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("durable history expands truthful output, pages older activity and opens an independent Output tab", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "durable-background-history.html?drive=1", "history-result")) as { count: number; expanded: string; unknown: string; active: string; output: string; calls: string[] };
  assert.equal(seen.count, 6);
  assert.match(seen.expanded, /Output truncated/);
  assert.match(seen.expanded, /Stopped by you/);
  assert.match(seen.expanded, /Duration unknown/);
  assert.match(seen.unknown, /No output snapshot was available/);
  assert.doesNotMatch(seen.unknown, /Success/);
  assert.equal(seen.active, "panel:output:history:failed");
  assert.match(seen.output, /test result: FAILED/);
  assert.ok(seen.calls.includes("readBackgroundHistoryOutput"));
});
