import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("the CLI's guess at the next message shows in the empty composer with Press Tab to accept, Tab takes it unsent, typing hides it until the field empties, and a working request shows none", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "composer-suggestion.html", "composer-suggestion-result", 60000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  assert.equal(seen.ghost, "Run the tests once more. Press Tab to accept");
  assert.equal(seen.chip, "Press Tab to accept");
  // The guess stands in for the placeholder, not on top of it.
  assert.ok(!seen.placeholder || seen.placeholder === "", `placeholder: ${String(seen.placeholder)}`);
  // The open conversation's composer is on screen: its CLI may guess.
  assert.deepEqual(seen.shownAtStart, [true]);
  assert.equal(seen.taken, "Run the tests once more.");
  assert.equal(seen.ghostAfterTab, false);
  assert.equal(seen.backWhenEmpty, true);
  assert.equal(seen.ghostWhileTyping, false);
  assert.equal(seen.ghostWhileWorking, false);
});
