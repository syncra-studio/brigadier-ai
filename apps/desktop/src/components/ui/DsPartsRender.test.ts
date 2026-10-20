import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("the merged small parts draw as one look each", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "ds-parts.html", "parts-result", 60000));

  // One meter bar: a muted capsule track 4 or 6 high, a rounded fill to its share; stacked parts
  // have square ends and a sliver still shows; waiting pulses a third; hidden unless it has a role.
  const { bars, radii } = seen;
  for (const look of [bars.sm, bars.md, bars.parts, bars.waiting, bars.empty]) {
    assert.equal(look.background, bars.track);
    assert.equal(look.radius, radii.capsule);
  }
  assert.deepEqual([bars.sm.height, bars.md.height], [4, 6]);
  assert.deepEqual(bars.sm.fills, [{ width: 40, radius: radii.capsule }]);
  assert.deepEqual([bars.sm.hidden, bars.md.hidden, bars.md.role], ["true", null, "meter"]);
  assert.equal(bars.md.fills[0].width, 120);
  assert.deepEqual(
    bars.parts.fills.map((fill: { width: number; radius: string }) => [fill.width, fill.radius]),
    [
      [100, radii.square],
      [2, radii.square],
    ],
  );
  assert.ok(Math.abs(bars.waiting.fills[0].width - 200 / 3) < 0.5, "a third of the track");
  assert.deepEqual(bars.empty.fills, []);

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
  const { search } = seen;
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
