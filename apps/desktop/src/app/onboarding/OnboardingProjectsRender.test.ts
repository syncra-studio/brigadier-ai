import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// The real step in Chromium over a faked daemon: nothing found in the history comes selected; a
// folder added by hand is selected but not counted as found in the history. After Select none
// (which then can't be pressed again, so it lets go of the focus), Enter continues without
// projects, as after any selection.
test("onboarding: nothing preselected, Enter continues after Select none", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "onboarding-projects.html?drive=1", "onboarding-projects-result", 20000)) as Record<string, unknown>;
  assert.deepEqual(seen, {
    first: "0 selected",
    header: "Found in your history·2",
    afterAdd: "1 selected",
    selected: "0 selected",
    asked: ["checkFolder", "updateSettings"],
  });
});
