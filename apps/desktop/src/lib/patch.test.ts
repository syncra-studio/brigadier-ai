import assert from "node:assert/strict";
import { test } from "node:test";

import { deletesFile, diffCounts, fileSides, parseDiff, singleFilePatch } from "@/lib/patch";

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

test("an edit rebuilt from its call becomes a patch with made-up hunks one line apart", () => {
  assert.equal(
    singleFilePatch("src/a.ts", "-const a = 1;\n+const a = 2;\n@@\n+added();"),
    "diff --git a/src/a.ts b/src/a.ts\n--- a/src/a.ts\n+++ b/src/a.ts\n@@ -1,1 +1,1 @@\n-const a = 1;\n+const a = 2;\n@@ -2,0 +3,1 @@\n+added();\n",
  );
});

test("git's hunks keep their numbers, and a deleted file stays deleted", () => {
  const diff = "diff --git a/x b/x\ndeleted file mode 100644\n--- a/x\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-a\n-b\n";
  assert.equal(singleFilePatch("x", diff), "diff --git a/x b/x\n--- a/x\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-a\n-b\n");
  assert.equal(deletesFile(diff), true);
  assert.equal(deletesFile("-a\n+b"), false);
});

test("a whole file's rows give both of its sides", () => {
  const rows = parseDiff("@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n").rows;
  assert.deepEqual(fileSides(rows), { before: "a\nb\nc\n", after: "a\nB\nc\n" });
});

test("a change of only the final newline keeps it on the side that lacks one", () => {
  const added = parseDiff("@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+b\n").rows;
  assert.deepEqual(fileSides(added), { before: "a\nb", after: "a\nb\n" });
  const removed = parseDiff("@@ -1,2 +1,2 @@\n a\n-b\n+b\n\\ No newline at end of file\n").rows;
  assert.deepEqual(fileSides(removed), { before: "a\nb\n", after: "a\nb" });
  const neither = parseDiff("@@ -1,2 +1,2 @@\n a\n-b\n+B\n\\ No newline at end of file\n").rows;
  assert.deepEqual(fileSides(neither), { before: "a\nb\n", after: "a\nB" });
});
