import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("Badge side padding uses the pill-x token", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "badge.html", "badge-result", 30000));
  assert.deepEqual(seen, { token: 8, left: 8, right: 8, height: 20 });
});
