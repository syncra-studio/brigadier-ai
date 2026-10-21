import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("the question card stays above the composer and the Job card only points at it; the rail holds the Workers strip alone, CI and the held merge being the Job card's", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "job-card-window.html", "job-card-window-result", 30000, { reducedMotion: true }));
  assert.equal(seen.error, undefined);
  // R3: the Job card points; the question, its words and its answers, are the card above the composer.
  assert.equal(seen.job, "Waiting for your answer below");
  assert.equal(seen.jobCopiesQuestion, false);
  assert.equal(seen.questionAboveField, true);
  // R2: CI's Open CI and the held merge's Cancel stay reachable, in the Job card, while it waits on you.
  assert.deepEqual(seen.jobButtons, ["Open CI", "Cancel"]);
  // The rail above the composer: the Workers strip only, unchanged.
  assert.equal(seen.railRows, 1);
  assert.equal(seen.rail, "1 worker workingStop all");
  assert.equal(seen.railHasCi, false);
  assert.equal(seen.railHasMerge, false);
});
