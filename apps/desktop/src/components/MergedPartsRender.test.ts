import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// Each group of look-alike parts was merged into one component; these read that one look back.
test("merged parts draw one look per group", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "merged-parts.html", "merged-parts-result", 30000));

  // Menu row: a 16px icon, a hint at the end, and a second line under the title when two-line.
  const [plain, hinted, twoLine] = seen.row.rows;
  assert.equal(plain.icon, seen.row.iconMd);
  assert.equal(hinted.icon, seen.row.iconMd);
  assert.equal(hinted.hint, "⌘1");
  assert.equal(twoLine.icon, null);
  assert.equal(twoLine.lines, 2);
  assert.ok(twoLine.height > plain.height);

  // Menu section label: mono capitals, or plain grey words in or out of a menu (a flyout's h2).
  const [caps, ...headings] = seen.label.labels;
  assert.deepEqual([caps.case, caps.mono], ["uppercase", true]);
  for (const heading of headings) {
    assert.deepEqual([heading.case, heading.mono, heading.fontSize, heading.color], ["none", false, seen.label.textSm, seen.label.muted]);
    assert.equal(heading.paddingTop, caps.paddingTop);
  }
  assert.equal(headings[1].tag, "h2");

  // Offer card: every offer is the same card, its icon in a square of the icon-button size.
  assert.equal(seen.offer.offers.length, 2);
  for (const offer of seen.offer.offers) {
    assert.equal(offer.radius, seen.offer.radius);
    assert.equal(offer.paddingTop, offer.paddingLeft);
    assert.equal(offer.tileWidth, seen.offer.tile);
  }

  // Mono box: one quiet code-surface box with no border, mono unless prose; a clamped one
  // offers Expand.
  assert.equal(seen.box.boxes.length, 3);
  for (const box of seen.box.boxes) {
    assert.equal(box.background, seen.box.surface);
    assert.equal(box.radius, seen.box.radius);
    assert.equal(box.border, 0);
  }
  assert.deepEqual(seen.box.boxes.map((box: { font: string }) => box.font === seen.box.mono), [true, true, false]);
  assert.deepEqual(seen.box.boxes.map((box: { expand: string[] }) => box.expand), [[], ["Expand"], []]);

  // Labelled divider: words between two 1px hairlines in the border colour, 8px above and below.
  assert.equal(seen.divider.dividers.length, 3);
  for (const divider of seen.divider.dividers) {
    assert.equal(divider.paddingTop, 8);
    assert.equal(divider.color, seen.divider.muted);
    assert.equal(divider.fontSize, seen.divider.textSm);
    assert.deepEqual(divider.rules.map((rule: { height: number }) => rule.height), [1, 1]);
    for (const rule of divider.rules) assert.equal(rule.background, seen.divider.border);
  }
});
