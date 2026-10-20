import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("the merged design-system parts draw as one look each", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "ds-merges.html", "merges-result", 60000));

  // A section heading carries its search name and a grey note beside its actions.
  assert.equal(seen.section.setting, "Disk use");
  assert.equal(seen.section.heading, "Disk use by project");
  assert.equal(seen.section.aside, seen.section.grey);

  // One card surface; its hairlines sit 16 in, whether its rows or the card pad the content.
  const { cards } = seen;
  for (const look of cards.surfaces) assert.deepEqual(look, cards.surfaces[0]);
  assert.equal(cards.rows, 17, "16 in, past the card's edge");
  assert.equal(cards.inset, cards.rows);
  assert.equal(cards.plain, "none");

  // Buttons with words are capsules; icon buttons keep the control corner.
  const { buttons } = seen;
  assert.equal(buttons.default.radius, buttons.radii.capsule);
  assert.equal(buttons.soft.radius, buttons.radii.capsule);
  assert.equal(buttons.softDestructive.radius, buttons.radii.capsule);
  assert.equal(buttons.icon.radius, buttons.radii.control);
  assert.notEqual(buttons.radii.capsule, buttons.radii.control);
  assert.equal(buttons.soft.background, buttons.colours.soft);
  assert.equal(buttons.softDestructive.background, buttons.colours.softDestructive);
  assert.equal(buttons.linkBlue.color, buttons.colours.link);
});
