import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("the merged design-system parts draw as one look each", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "ds-merges.html", "merges-result", 60000));

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
