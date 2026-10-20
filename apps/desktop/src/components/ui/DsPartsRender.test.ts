import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("the merged small parts draw as one look each", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "ds-parts.html", "parts-result", 60000));

  // One segmented control: words or tipped icons, each a named radio; the chosen one filled, 24px high.
  const { segmented } = seen;
  assert.deepEqual(
    segmented.icons.map((item: { name: string }) => item.name),
    ["Focused view", "Canvas view"],
  );
  for (const item of [...segmented.words, ...segmented.icons]) {
    assert.equal(item.role, "radio");
    assert.equal(item.height, 24);
    assert.equal(item.fill === segmented.chosen, item.on, `${item.name} filled only when chosen`);
  }

  // One inspector dropdown: the choice, else its placeholder; with nothing to pick it is off.
  const { picker } = seen;
  assert.equal(picker.chosen.text, "seven_day");
  assert.equal(picker.chosen.disabled, false);
  assert.equal(picker.unpicked.text, "Pick…");
  assert.equal(picker.unpicked.justify, "flex-start");
  assert.equal(picker.empty.disabled, true);
});
