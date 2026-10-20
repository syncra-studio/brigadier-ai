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

  // One search field. Settings' pill: a label box, a search input with a clear button, Esc empties
  // it and stops there. A picker's bordered field keeps Esc for the picker. With buttons after the
  // input the box is a plain div and the input keeps its own name.
  const { search, radii } = seen;
  assert.deepEqual(
    [search.before.pill.box, search.before.pill.type, search.before.pill.radius, search.before.pill.clear],
    ["label", "search", radii.capsule, true],
  );
  assert.deepEqual(
    [
      search.before.picker.box,
      search.before.picker.type,
      search.before.picker.radius,
      search.before.picker.clear,
    ],
    ["label", "text", radii.control, false],
  );
  assert.equal(search.before.pill.height, 32);
  assert.equal(search.before.picker.height, search.before.panel.height);
  assert.deepEqual(
    [search.before.panel.box, search.before.panel.icon, search.before.panel.buttons],
    ["div", false, ["Match case"]],
  );
  assert.equal(search.after.pill.value, "");
  assert.equal(search.after.picker.value, "brig");
  assert.deepEqual(search.heard, ["Search projects"]);

  // One inspector dropdown: the choice, else its placeholder; with nothing to pick it is off.
  const { picker } = seen;
  assert.equal(picker.chosen.text, "seven_day");
  assert.equal(picker.chosen.disabled, false);
  assert.equal(picker.unpicked.text, "Pick…");
  assert.equal(picker.unpicked.justify, "flex-start");
  assert.equal(picker.empty.disabled, true);
});
