import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// The real step in Chromium over a faked daemon: everything found in the history comes ticked;
// a folder added by hand comes ticked too but isn't counted as found in the history. One button
// ticks the rest or clears them all, its label following the cards, and it stays put. With none
// ticked the action continues without projects, and Enter on a card presses it, not the card.
// The folder picker is readied as the step opens.
test("onboarding: projects come ticked, one button selects all or none", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "onboarding-projects.html?drive=1", "onboarding-projects-result", 20000)) as Record<string, unknown>;
  assert.deepEqual(seen, {
    first: { count: "2 of 2 selected", toggle: "Select none", action: "Add 2 projects↩" },
    header: "Found in your history2",
    afterAdd: { count: "3 of 3 selected", toggle: "Select none", action: "Add 3 projects↩" },
    unticked: { count: "2 of 3 selected", toggle: "Select all", action: "Add 2 projects↩" },
    all: { count: "3 of 3 selected", toggle: "Select none", action: "Add 3 projects↩" },
    none: { count: "0 of 3 selected", toggle: "Select all", action: "Continue without projects↩", moved: false },
    enter: { billing: "false" },
    asked: ["warm_folder_picker", "checkFolder", "updateSettings"],
  });
});
