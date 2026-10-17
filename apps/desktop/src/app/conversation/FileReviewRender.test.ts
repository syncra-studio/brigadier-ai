import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Look = { changes: string | null; files: string | null; buttons: string[]; bars: string[][]; current: number; diff: boolean };

test("a file tab reviews its agents' changes: Keep and Undo per change, the bar's places and file buttons, then the next file", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "file-review.html", "file-review-result", 90000)) as {
    error?: string; first: Look; kept: Look; stepped: number; done: Look; next: string | null; asked: string[]; base: string; current: string;
  };
  assert.equal(seen.error, undefined);
  // The bar: the change and the file among the rest, then Undo File and Keep File with its key.
  assert.deepEqual([seen.first.changes, seen.first.files, seen.first.diff], ["1 of 3", "1 of 2 Files", true]);
  assert.deepEqual(seen.first.buttons, ["Undo File", "Keep File ⌘⏎"]);
  // Each change has Undo and Keep; the one in view names their keys.
  assert.deepEqual(seen.first.bars, [["Undo ⌘N", "Keep ⌘Y"], ["Undo", "Keep"], ["Undo", "Keep"]]);
  assert.equal(seen.first.current, 0);
  // ⌘Y keeps the first change; the second steps into view and its Undo undoes it.
  assert.equal(seen.kept.changes, "1 of 2");
  assert.equal(seen.stepped, 1);
  assert.deepEqual(seen.asked, [
    "keepChanges src/cart.ts h1",
    "undoPendingChanges src/cart.ts h3",
    "keepChanges src/cart.ts h2",
  ]);
  assert.equal(seen.base, "export const RATE = 0.2;\n\nexport function tax(amount: number): number {\n  return amount * RATE;\n}\n");
  assert.equal(seen.current, seen.base);
  // Done: no changes left here, and the bar offers the rest.
  assert.deepEqual(seen.done.buttons, ["Keep all changes", "Review next file"]);
  assert.equal(seen.next, "file:src/total.ts");
});
