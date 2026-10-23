import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("a session's model picker continues on the other CLI from its list: on the rules' model, or on a picked one (S38c)", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "composer-continue-on.html", "composer-continue-on-result", 90000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  // Its own CLI's list has only models.
  assert.deepEqual(seen.claude, ["Opus 5.5Default"]);
  // The other CLI's list opens with the move, then its models.
  assert.deepEqual(seen.codex, [
    ["model-selector-continue", "Continue on CodexFresh context, with a hand-off note"],
    ["model-selector-item", "GPT-5.5"],
  ]);
  assert.deepEqual(seen.asked, [
    { provider: "codex", model: null },
    { provider: "codex", model: { provider: "codex", model: "gpt-5.5", effort: "medium" } },
  ]);
});
