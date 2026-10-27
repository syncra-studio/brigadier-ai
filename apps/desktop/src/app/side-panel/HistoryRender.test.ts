import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("durable history expands truthful output, pages older activity and opens an independent Output tab", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "durable-background-history.html?drive=1", "history-result")) as { count: number; expanded: string; unknown: string; active: string; output: string; calls: string[]; initialTotal: string; liveOutput: string; historyRequests: boolean[]; refreshed: string };
  assert.equal(seen.count, 5);
  assert.equal(seen.initialTotal, "Everything that ran this session · 5");
  assert.doesNotMatch(seen.expanded, /Watch the pull request's CI|Paused docs build/);
  assert.equal(seen.liveOutput, "running", "Output fallback can resolve a live record");
  assert.equal(seen.historyRequests.filter(Boolean).length, 1, "only Output resolution opts into live records");
  assert.match(seen.refreshed, /Everything that ran this session · 6/);
  assert.match(seen.refreshed, /Watch the pull request's CI/);
  assert.doesNotMatch(seen.refreshed, /Paused docs build/);
  assert.match(seen.expanded, /Output truncated/);
  assert.match(seen.expanded, /Stopped by you/);
  assert.match(seen.expanded, /Duration unknown/);
  assert.match(seen.unknown, /No output snapshot was available/);
  assert.doesNotMatch(seen.unknown, /Success/);
  assert.equal(seen.active, "panel:output:history:failed");
  assert.match(seen.output, /test result: FAILED/);
  assert.ok(seen.calls.includes("readBackgroundHistoryOutput"));
});
