import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// The real page in Chromium: search by words, and key presses recorded before the app's own
// handlers see them (a menu's echo too), Esc cancelling.
test("keyboard shortcuts search by words and by keystrokes", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "shortcuts.html?drive=1", "shortcuts-result", 12000)) as Record<
    string,
    unknown
  >;
  assert.equal(seen.count, 68);
  assert.deepEqual(seen.chips, ["⌘B", "⇧⌘S"]);
  assert.deepEqual(seen.byWords, ["Dictate"]);
  assert.equal(seen.none, "No shortcuts match");
  assert.equal(seen.recording, "Press a shortcut… Esc cancels.");
  assert.deepEqual(seen.byPress, ["Back"]);
  assert.deepEqual(seen.pressChip, ["⌘["]);
  // Recorded keys, the menu's copy of one, Esc and a lone modifier reach none of the app's handlers.
  assert.deepEqual(seen.reachedWhileRecording, []);
  assert.deepEqual(seen.afterEsc, { rows: 68, input: true });
  assert.deepEqual(seen.clash, ["Toggle sidebar", "Save file as"]);
  assert.deepEqual(seen.reachedAfterRecording, []);
  assert.deepEqual(seen.reachedLater, ["⌘["]);
  // The shell holds the menu's own items for each recording, and lets go after it.
  assert.deepEqual(seen.capture, [true, false, true, false, true, false]);
});
