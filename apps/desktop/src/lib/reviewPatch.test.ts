import assert from "node:assert/strict";
import { test } from "node:test";

import { changeLine, reviewPatch } from "@/lib/reviewPatch";

const hunk = (oldStart: number, oldLines: number, newStart: number, newLines: number) => ({ id: `${oldStart}`, oldStart, oldLines, newStart, newLines });

test("a file under review is one patch over all of it, each change where the engine numbers it", () => {
  const hunks = [hunk(0, 0, 1, 1), hunk(2, 1, 2, 0), hunk(4, 1, 4, 1)];
  assert.equal(
    reviewPatch("f.txt", "one\ntwo\nthree\nfour\n", "zero\none\nthree\nFOUR\n", hunks),
    ["--- a/f.txt", "+++ b/f.txt", "@@ -1,4 +1,4 @@", "+zero", " one", "-two", " three", "-four", "+FOUR", ""].join("\n"),
  );
  assert.deepEqual(hunks.map(changeLine), [
    { side: "additions", lineNumber: 1 },
    { side: "deletions", lineNumber: 2 },
    { side: "deletions", lineNumber: 4 },
  ]);
});

test("an added file, a deleted one and a last line without its ending read as git writes them", () => {
  assert.equal(reviewPatch("n.txt", null, "a\nb", [hunk(0, 0, 1, 2)]), "--- a/n.txt\n+++ b/n.txt\n@@ -0,0 +1,2 @@\n+a\n+b\n\\ No newline at end of file\n");
  assert.equal(reviewPatch("d.txt", "a\n", null, [hunk(1, 1, 0, 0)]), "--- a/d.txt\n+++ b/d.txt\n@@ -1,1 +0,0 @@\n-a\n");
  assert.equal(
    reviewPatch("e.txt", "a\nb", "a\nc\n", [hunk(2, 1, 2, 1)]),
    "--- a/e.txt\n+++ b/e.txt\n@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+c\n",
  );
});
