import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("at 1280×820 a pending question card's bottom edge stays clear of the composer", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "question-card-edge.html", "question-card-edge-result", 60000)) as {
    error?: string;
    window: number[];
    cardBottom: number;
    composerTop: number;
    border: number;
    radius: number;
  };
  assert.equal(seen.error, undefined, seen.error ?? "");
  assert.deepEqual(seen.window, [1280, 820]);
  // The card ends a small gap above the composer, with its bottom edge and lower corners drawn.
  assert.ok(seen.cardBottom + 4 <= seen.composerTop, `the card ends at ${seen.cardBottom}, the composer's top is at ${seen.composerTop}`);
  assert.ok(seen.border > 0, "its bottom border is drawn");
  assert.ok(seen.radius > 0, "its lower corners are rounded");
});
