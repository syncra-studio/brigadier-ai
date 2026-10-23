import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("the model picker shows what the rules keep out with the rule's words, never stays on it, and a refused Send keeps the message (issue 545)", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "composer-rules.html", "composer-rules-result", 90000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  // Left on Max before the rule: the picker is on High.
  assert.equal(seen.trigger, "Opus 5.5High");
  assert.deepEqual(seen.efforts, [
    { text: "Low", disabled: false, checked: false },
    { text: "Medium", disabled: false, checked: false },
    { text: "HighDefault", disabled: false, checked: true },
    { text: "Extra highNever go above High effort", disabled: true, checked: false },
    { text: "MaxNever go above High effort", disabled: true, checked: false },
  ]);
  assert.equal(seen.created, 1);
  assert.equal(seen.banner, "Refused by your rule “Never go above High effort”");
  assert.equal(seen.kept, "Reply with the single word OK.");
});
