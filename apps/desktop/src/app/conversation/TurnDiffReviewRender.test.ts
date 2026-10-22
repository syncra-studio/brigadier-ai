import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Look = { title: string | null; review: string | null; buttons: (string | null)[]; rows: [string | null, string | null][] };

test("an answer's card reviews its files: Undo All, Keep All, Review Changes, per-file Undo and Keep, then what was kept, undone and merged", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "turn-diff-review.html", "turn-diff-review-result", 90000)) as {
    error?: string; waiting: Look; opened: string | null; keptOne: Look; done: Look; merged: Look; putBack: Look; putBackAlone: Look; asked: string[];
  };
  assert.equal(seen.error, undefined);
  assert.deepEqual([seen.waiting.title, seen.waiting.review], ["3 Files", "2 to review"]);
  assert.deepEqual(seen.waiting.buttons, ["Undo All", "Keep All", "Review Changes", "Fold the files"]);
  assert.deepEqual(seen.waiting.rows, [["pending", "Undo Keep"], ["kept", "Kept"], ["pending", "Undo Keep"]]);
  // Review Changes opens the first file that waits, in its own tab.
  assert.equal(seen.opened, "file:src/a.ts");
  assert.deepEqual(seen.keptOne.rows.map(([state]) => state), ["kept", "kept", "pending"]);
  assert.deepEqual(seen.asked, ["keepChanges src/a.ts", "undoPendingChanges src/c.ts"]);
  // Done: the card stays as the turn's record, its rows each opening their diff.
  // Every file decided, the turn keeps Discard, as the design's reviewed tally (app.pen kUqk1 › wYSCe) has it.
  assert.deepEqual([seen.done.title, seen.done.review, seen.done.buttons], ["Edited 3 files", "kept 2, undid 1", ["Revert", "Fold the files"]]);
  assert.deepEqual(seen.done.rows, [["kept", "Kept"], ["kept", "Kept"], ["undone", "Undone"]]);
  // A merge took the files that waited (#288): no Undo or Keep; the merge's own record says it
  // merged, once, so no row says Merged.
  assert.deepEqual([seen.merged.title, seen.merged.review, seen.merged.buttons], ["Edited 3 files", "kept 1", ["Revert", "Fold the files"]]);
  assert.deepEqual(seen.merged.rows, [["merged", null], ["kept", "Kept"], ["merged", null]]);
});
