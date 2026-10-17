import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("Find clears and aligns its marks and restores folded messages", { timeout: 180000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "thread-find.html?drive=glitches", "thread-find-result", 150000));
  assert.equal(seen.error, undefined);
  await t.test("button text cannot leave misplaced highlight strips", () => {
    assert.equal(seen.buttonMarks, 0, "button ranges must stay out of the highlight registry");
    assert.equal(seen.previousGone, true);
  });
  await t.test("inline code has its own text-aligned mark", () => {
    assert.deepEqual(seen.inline, { split: true, aligned: true });
  });
  await t.test("the changed-file ring stays inside its row", () => {
    assert.deepEqual(seen.ring, { onRow: true, inset: true });
  });
  await t.test("Esc collapses the message Find expanded", () => {
    assert.equal(seen.expanded, "true");
    assert.equal(seen.collapsed, "false");
  });
});
