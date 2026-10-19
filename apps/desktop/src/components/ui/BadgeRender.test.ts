import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("Badge side padding uses the pill-x token at each density", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "badge.html", "badge-result", 30000));
  assert.deepEqual(seen, [
    { density: "normal", token: 8, left: 8, right: 8, height: 20 },
    { density: "compact", token: 6, left: 6, right: 6, height: 16 },
  ]);
});
