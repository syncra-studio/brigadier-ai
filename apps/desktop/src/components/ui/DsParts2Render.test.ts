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

  // One pager: bare arrows (control corners, no fill) around grey 12px "n of m"; an end's arrow is off.
  const { pager } = seen;
  assert.equal(pager.round.text, "1 of 3");
  assert.equal(pager.files.text, "2 of 4 Files");
  assert.equal(pager.bare.text, "");
  assert.equal(pager.round.size, colours.xs);
  assert.equal(pager.round.color, pager.look.muted);
  assert.deepEqual(
    pager.round.arrows.map((arrow: { label: string; disabled: boolean }) => [arrow.label, arrow.disabled]),
    [["Previous question", true], ["Next question", false]],
  );
  assert.deepEqual(pager.bare.arrows.map((arrow: { label: string }) => arrow.label), ["Previous conflict", "Next conflict"]);
  for (const arrow of [...pager.round.arrows, ...pager.bare.arrows]) {
    assert.equal(arrow.radius, pager.look.control);
    assert.equal(arrow.background, pager.look.none);
    assert.equal(arrow.icon, pager.look.icon);
  }

  // One key hint: the grey mono key, or the chip (a capsule tinted with its own text colour, in sans 12),
  // which is also how a key on a button draws.
  const { keys } = seen;
  assert.notEqual(keys.key.radius, keys.look.capsule);
  assert.equal(keys.chip.radius, keys.look.capsule);
  assert.equal(keys.chip.background, keys.look.tint);
  assert.equal(keys.chip.color, keys.look.grey);
  assert.equal(keys.chip.font, keys.look.sans);
  assert.equal(keys.chip.size, colours.xs);
  assert.equal(keys.onButton.radius, keys.look.capsule);
  assert.equal(keys.onButton.font, keys.look.sans);
});
