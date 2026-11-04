import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("the model pill says the effort a model runs at, Default for an OpenCode model with none picked (issue 670)", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "model-pill-effort.html", "model-pill-effort-result", 90000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  assert.equal(seen.claude, "Opus 5.5High");
  assert.equal(seen.opencode, "GLM-5.3Default");
  assert.equal(seen.opencodeHigh, "GLM-5.3High");
  // A model without efforts has no effort word.
  assert.equal(seen.noEfforts, "Big Pickle");
});
