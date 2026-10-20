import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("the merged small parts (11–14) draw as one look each", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "ds-parts-2.html", "parts2-result", 60000));

  // A diff stat: mono 12, "+N −N" a space apart, green and red; a zero, `dim` and `quiet` stay grey.
  const { counts } = seen;
  const { colours } = counts;
  assert.equal(counts.plain.font, colours.mono);
  assert.equal(counts.plain.size, colours.xs);
  assert.equal(counts.plain.text, "+88 −7");
  assert.equal(counts.plain.plus, colours.success);
  assert.equal(counts.plain.minus, colours.destructive);
  assert.deepEqual([counts.zero.plus, counts.zero.minus], [colours.success, colours.muted]);
  assert.deepEqual([counts.dim.plus, counts.dim.minus], [colours.muted, colours.muted]);
  assert.deepEqual([counts.quiet.plus, counts.quiet.minus], [colours.muted, colours.muted]);

  // One copy button: its tip names it, then a check and "Copied"; a click it stops never reaches the card.
  const { copy } = seen;
  assert.deepEqual(copy.before, ["Copy command", "Copy plan"]);
  assert.deepEqual(copy.after, ["Copied", "Copied"]);
  assert.deepEqual(copy.checked, [true, true]);
  assert.deepEqual(copy.copied, ["npm test", "# Plan"]);
  assert.equal(copy.cardClicks, 0);
});
