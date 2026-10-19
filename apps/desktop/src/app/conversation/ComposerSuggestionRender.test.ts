import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("the guess at the next message shows in the empty composer with no key hint, Tab, → and Enter take it unsent, typing hides it until the field empties, and a working request shows none", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "composer-suggestion.html", "composer-suggestion-result", 60000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  // Just the guess: no "Press Tab to accept" with it.
  assert.equal(seen.ghost, "Run the tests once more.");
  // The guess stands in for the placeholder, not on top of it.
  assert.ok(!seen.placeholder || seen.placeholder === "", `placeholder: ${String(seen.placeholder)}`);
  assert.equal(seen.taken, "Run the tests once more.");
  assert.equal(seen.ghostAfterTab, false);
  assert.equal(seen.backWhenEmpty, true);
  assert.equal(seen.ghostAfterTaken, "Run the tests once more.");
  assert.deepEqual(seen.takesUnsent, { ArrowRight: true, Enter: true });
  assert.equal(seen.ghostWhileTyping, false);
  assert.equal(seen.ghostWhileWorking, false);
});

test("the composer keeps typed text as typed: no spell checking, auto-correct or capitals, and two spaces stay two spaces", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "composer-suggestion.html", "composer-suggestion-result", 60000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  assert.deepEqual(seen.plainTyping, ["false", "off", "off"]);
  assert.equal(seen.doubleSpace, "a  ");
});
