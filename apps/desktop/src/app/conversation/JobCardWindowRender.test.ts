import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("a plain session's window shows no Job card: the question card stays above the composer, CI is no Background row, the rail holds the Workers strip alone", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "job-card-window.html", "job-card-window-result", 30000, { reducedMotion: true }));
  assert.equal(seen.error, undefined);
  // Outside a plan or an overnight run there is no Job card, the thread's headline notwithstanding.
  assert.equal(seen.jobCards, 0);
  // R3: the question, its words and its answers, are the card above the composer.
  assert.equal(seen.questionAboveField, true);
  // Background is the CLI's own tasks only (S14): the session's CI watch is no row of it.
  assert.equal(seen.ciRow, false);
  // The rail above the composer: the Workers strip only, unchanged.
  assert.equal(seen.railRows, 1);
  assert.equal(seen.rail, "1 worker workingStop all");
  assert.equal(seen.railHasCi, false);
});
