import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("the Job card: its line, a bar of one segment per step, the step at work before the next, a failure in red, its own card in the column", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "job-card.html", "job-card-result", 30000, { reducedMotion: true }));
  assert.deepEqual(seen.working, {
    cards: ["context", "job"],
    label: "Reviewing · 3 of 5",
    segments: ["done", "active", "pending", "pending", "pending"],
    // A passed step fills its segment, the one at work as far as it got (3 of 5), the rest are empty.
    fills: ["100%", "60%", "", "", ""],
    // One line under the bar: the step at work, shimmering, then the next.
    line: ["active:Reviewing", "next:Fixing"],
    detail: null,
    shimmer: ["Reviewing"],
    // One step of five passed, three fifths of the next: 32%.
    bar: "32",
    red: false,
    schedule: null,
  });
  // A failed step: its segment red, its name first on the line in red, its words under it.
  assert.deepEqual(seen.failed.segments, ["done", "done", "failed", "active", "pending"]);
  assert.deepEqual(seen.failed.fills, ["100%", "100%", "100%", "", ""]);
  assert.deepEqual(seen.failed.line, ["failed:Checks", "next:Fixing"]);
  assert.equal(seen.failed.detail, "2 tests failed in the desktop app");
  assert.deepEqual(seen.failed.shimmer, []);
  assert.equal(seen.failed.label, "Checks failed · fixing");
  assert.equal(seen.failed.red, true);
  assert.equal(seen.phased.label, "Phase 2 of 4 · Reviewing");
  assert.deepEqual(seen.scheduled.cards, ["context", "schedule"]);
  assert.equal(seen.scheduled.schedule, "Resumes at 4:00 AMin 2h 10mwhen the Claude limit resets");
  // With the context card hidden, the job card takes its place at the top.
  assert.deepEqual(seen.hidden.cards, ["job", "schedule"]);
  // Finished work leaves no card.
  assert.deepEqual(seen.done.cards, ["context"]);
  assert.deepEqual(seen.tab.steps, ["Building", "Reviewing", "Checks failed", "Fixing", "Merging"]);
  assert.deepEqual(seen.tab.current, [null, null, null, "step", null]);
  assert.equal(seen.tab.findings, 3);
});
