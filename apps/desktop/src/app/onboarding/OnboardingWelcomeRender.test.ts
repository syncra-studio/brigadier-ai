import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// The first launch's intro greets with its title on two lines, read out as one.
test("onboarding: the intro's title", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "onboarding-welcome.html", "onboarding-welcome-result", 10000)) as Record<string, unknown>;
  assert.deepEqual(seen, { name: "Brigadier, at your command.", lines: ["Brigadier,", "at your command."] });
});
