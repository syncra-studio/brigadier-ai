import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

const range = (from: number, to: number) => Array.from({ length: to - from + 1 }, (_, index) => String(from + index));

type Look = { lines: number; numbers: string[]; folds: string[]; marks: string[]; split: number; overflow: string | null; coloured: boolean };

test("the Review tab's diff: numbered and coloured, folds that open, word diffs, wrap, split, and Add to chat", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "review-diff.html", "review-diff-result", 90000)) as Record<
    "unified" | "expanded" | "wrapped" | "split",
    Look
  > & { error?: string; plus: boolean; composer: string };
  assert.equal(seen.error, undefined);
  // Unified: each change with three lines around it, both numbers of a replaced line, the
  // stretch between them folded with its count, all in the file's colours.
  assert.deepEqual(seen.unified.numbers, [...range(1, 3), "3", ...range(4, 7), ...range(21, 25), "25", ...range(26, 30)]);
  assert.deepEqual(seen.unified.folds, ["13 unmodified lines"]);
  assert.equal(seen.unified.coloured, true);
  // The fold opens in place: every line shown, nothing folded.
  assert.deepEqual([seen.expanded.lines, seen.expanded.folds], [32, []]);
  // Word diffs: only the words a replaced line changed.
  assert.deepEqual(seen.unified.marks.slice(0, 2), ["Math.round(", " * 100) / 100"]);
  // A long line scrolls, then wraps.
  assert.deepEqual([seen.unified.overflow, seen.wrapped.overflow], ["scroll", "wrap"]);
  // Split: the old file and the new one side by side, each numbered from 1.
  assert.deepEqual([seen.split.split, seen.split.numbers], [2, [...range(1, 30), ...range(1, 30)]]);
  // Add to chat: the "+" by the hovered line's number puts the line, quoted, into the composer.
  assert.equal(seen.plus, true);
  assert.equal(seen.composer, "`src/values.ts` line 3:\n> export const value3 = Math.round(3 * 100) / 100;\n\n");
});

// Warmed at idle (#410), View changes draws the diff in its colours at once: no plain lines
// first, and no moment waiting for the diff engine or its highlighter.
test("warmed, the Review tab's diff draws in its colours at once", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "review-diff.html?warm=1", "review-diff-result", 90000)) as { error?: string; plain: boolean; firstColoured: boolean | null };
  assert.equal(seen.error, undefined);
  assert.deepEqual(seen, { plain: false, firstColoured: true });
});
