import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Seen = {
  widths: number[];
  tableFits: boolean;
  lastHeaderClear: boolean;
  width: number;
  broken: string[];
  scrolls: boolean;
};

// #402: an import path and a URL in an answer's table wrap where they must, so the last column
// ("Best for") keeps a readable width instead of a word per line, the table fits its width, and
// the hover buttons never sit over the last header.
test("an answer's table keeps every column readable and its last header clear of the hover buttons", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "answer-table.html", "answer-table-result", 5000)) as Seen;
  assert.equal(seen.widths.length, 5);
  assert.ok(seen.width > 0 && seen.width <= 900, `column ${seen.width}`);
  assert.ok(seen.widths.every((width) => width >= 96), `widths ${seen.widths.join(", ")}`);
  assert.ok(seen.widths.at(-1)! >= 140, `last column ${seen.widths.at(-1)}`);
  assert.equal(seen.tableFits, true);
  assert.equal(seen.lastHeaderClear, true);
  assert.deepEqual(seen.broken, []);
});

// #402 again: with a side panel open the chat column is narrow. No header or cell word breaks
// mid-word ("Be / st / for"); a table too wide for the column scrolls sideways instead.
test("an answer's table in a narrow column never breaks a word and scrolls sideways", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "answer-table.html?narrow=1", "answer-table-result", 5000)) as Seen;
  assert.deepEqual(seen.broken, []);
  assert.equal(seen.scrolls, true, JSON.stringify(seen));
  assert.ok(seen.widths.every((width) => width >= 96), `widths ${seen.widths.join(", ")}`);
});
