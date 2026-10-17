import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// The real tooltips in Chromium, with the real pointer: from a control onto its tooltip, then
// onto the next control, the next control's tooltip shows and the first one's is gone.
test("tooltips: each hover shows its own control's tooltip", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "tooltips.html?drive=1", "tooltips-result", 20000)) as Record<string, unknown>;
  assert.deepEqual(seen, {
    onFirst: ["The first control’s tooltip, long enough to reach past the next one"],
    onSecond: ["The second control’s tooltip"],
  });
});
