import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("the Job card: its line, one bar and its steps listed, a failure in red, its own card in the column", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "job-card.html", "job-card-result", 30000, { reducedMotion: true }));
  assert.deepEqual(seen.working, {
    cards: ["context", "job"],
    label: "Reviewing · 3 of 5",
    stages: ["done", "active", "pending", "pending", "pending"],
    names: ["Building", "Reviewing", "Fixing", "Checks", "Merging"],
    // A passed step is checked, the rest wait with a dim check; only the step at work shimmers.
    marks: ["done", "active", "pending", "pending", "pending"],
    shimmer: ["Reviewing"],
    // One step of five passed, three fifths of the next: 32%.
    bar: "32",
    red: false,
    schedule: null,
  });
  assert.deepEqual(seen.failed.marks, ["done", "done", "cross", "active", "pending"]);
  assert.equal(seen.failed.label, "Checks failed · fixing");
  assert.deepEqual(seen.failed.stages, ["done", "done", "failed", "active", "pending"]);
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
