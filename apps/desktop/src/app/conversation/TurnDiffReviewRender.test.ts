import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Look = { title: string | null; review: string | null; buttons: (string | null)[]; rows: [string | null, string | null][] };

test("an answer's card reviews its files: Undo All, Keep All, Review Changes, per-file Undo and Keep, then what was kept and undone", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "turn-diff-review.html", "turn-diff-review-result", 90000)) as {
    error?: string; waiting: Look; opened: string | null; keptOne: Look; done: Look; asked: string[];
  };
  assert.equal(seen.error, undefined);
  assert.deepEqual([seen.waiting.title, seen.waiting.review], ["3 Files", "· 2 to review"]);
  assert.deepEqual(seen.waiting.buttons, ["View changed files", "Undo All", "Keep All", "Review Changes"]);
  assert.deepEqual(seen.waiting.rows, [["pending", "Undo Keep"], ["kept", "Kept"], ["pending", "Undo Keep"]]);
  // Review Changes opens the first file that waits, in its own tab.
  assert.equal(seen.opened, "file:src/a.ts");
  assert.deepEqual(seen.keptOne.rows.map(([state]) => state), ["kept", "kept", "pending"]);
  assert.deepEqual(seen.asked, ["keepChanges src/a.ts", "undoPendingChanges src/c.ts"]);
  // Done: the card stays as the turn's record, with no buttons.
  assert.deepEqual([seen.done.title, seen.done.review, seen.done.buttons], ["Edited 3 files", "· kept 2, undid 1", ["View changed files"]]);
  assert.deepEqual(seen.done.rows, [["kept", "Kept"], ["kept", "Kept"], ["undone", "Undone"]]);
});
