import assert from "node:assert/strict";
import { test } from "node:test";

import { diffCounts, parseDiff, wordSpans } from "@/lib/patch";

const strip = (rows: ReturnType<typeof parseDiff>["rows"]) =>
  rows.map((row) => (row.kind === "gap" ? `gap ${row.count}` : `${row.kind} ${row.old ?? "-"} ${row.new ?? "-"} ${row.text}`));

test("a file's diff with hunk headers numbers its lines and counts the stretch between hunks", () => {
  const diff = "--- a/math.js\n+++ b/math.js\n@@ -2,3 +2,3 @@\n a\n-b\n+B\n c\n@@ -20,2 +20,3 @@\n x\n+y\n z\n";
  const { numbered, rows } = parseDiff(diff);
  assert.equal(numbered, true);
  assert.deepEqual(strip(rows), [
    "context 2 2 a", "del 3 - b", "add - 3 B", "context 4 4 c", "gap 15", "context 20 20 x", "add - 21 y", "context 21 22 z",
  ]);
  assert.deepEqual(diffCounts(rows), { added: 2, removed: 1 });
});

test("an edit rebuilt from its call has no numbers", () => {
  const { numbered, rows } = parseDiff("-old line\n+new line\n+another");
  assert.equal(numbered, false);
  assert.deepEqual(strip(rows), ["del - - old line", "add - - new line", "add - - another"]);
});

const mark = (text: string, ranges: [number, number][]) => ranges.map(([from, to]) => text.slice(from, to));

test("a word diff marks only the words that changed, spaces between changed words joined", () => {
  const spans = wordSpans("return Math.round(value * 100) / 100;", "return Math.round((value + Number.EPSILON) * 100) / 100;");
  assert.deepEqual(mark("return Math.round(value * 100) / 100;", spans.before), []);
  assert.deepEqual(mark("return Math.round((value + Number.EPSILON) * 100) / 100;", spans.after), ["(", "+ Number.EPSILON)"]);
  const words = wordSpans("const total = sum(a, b);", "let total = add(a, b, c);");
  assert.deepEqual(mark("const total = sum(a, b);", words.before), ["const", "sum"]);
  assert.deepEqual(mark("let total = add(a, b, c);", words.after), ["let", "add", ", c"]);
});
