import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// The real step in Chromium over a faked daemon: after Select none (which then can't be pressed
// again, so it lets go of the focus), Enter continues without projects, as after any selection.
test("onboarding: Enter continues after Select none", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "onboarding-projects.html?drive=1", "onboarding-projects-result", 20000)) as Record<string, unknown>;
  assert.deepEqual(seen, { selected: "0 selected", asked: ["updateSettings"] });
});
