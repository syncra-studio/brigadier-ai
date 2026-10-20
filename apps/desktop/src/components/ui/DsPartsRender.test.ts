import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("the merged small parts draw as one look each", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "ds-parts.html", "parts-result", 60000));

  // One inspector dropdown: the choice, else its placeholder; with nothing to pick it is off.
  const { picker } = seen;
  assert.equal(picker.chosen.text, "seven_day");
  assert.equal(picker.chosen.disabled, false);
  assert.equal(picker.unpicked.text, "Pick…");
  assert.equal(picker.unpicked.justify, "flex-start");
  assert.equal(picker.empty.disabled, true);
});
