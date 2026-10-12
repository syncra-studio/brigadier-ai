import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage } from "@/fixtures/headless";

test("Search streams grouped results, opens a line, asks again on its options, replaces after a confirm and clears; many results are drawn only in view and move by keyboard", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "search.html", "search-result", 8000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  assert.equal(seen.starts, 1, "one search once typing pauses");
  assert.deepEqual(seen.query, { text: "foo", matchCase: false, wholeWord: false, regex: false, include: "", exclude: "" });
  assert.deepEqual(seen.groups, [
    { name: "app.ts", count: "2", marks: ["foo", "foo"] },
    { name: "README.md", count: "1", marks: ["foo"] },
  ], "a batch sent before the search's id is known still shows, in order");
  assert.deepEqual(seen.opened, [["src/app.ts", 3, false]], "a line opens its file there, in the preview tab");
  assert.equal(seen.matchCase, true, "an option asks again");
  assert.equal(seen.replaceTypingSearched, false, "typing a replacement doesn't search");
  assert.equal(seen.fileConfirm, "Replace 2 matches in 1 file?");
  assert.equal(seen.noReplaceBeforeConfirm, true);
  assert.equal(seen.confirm, "Replace 3 matches in 2 files?");
  assert.deepEqual(seen.replace, {
    replacement: "bar", matchCase: true,
    files: [{ path: "src/app.ts", hash: "h-app", matches: 2 }, { path: "README.md", hash: "h-readme", matches: 1 }],
  }, "the files go with the search's hashes and counts");
  assert.match(String(seen.outcome), /Replaced 2 matches in 1 file\..*README\.md \(it changed since the search\)/);
  assert.equal(seen.reconnected, false, "after reconnecting, the search shown is asked again");
  assert.match(String(seen.patternError), /valid regular expression/);
  assert.equal(seen.cleared, true, "a cleared query clears the results");
  assert.equal(seen.waitingReplace, false, "no Replace before the session's own copy exists");
  assert.match(String(seen.waitingNote), /own copy/);
  assert.ok(Number(seen.manyDrawn) < 100, `only the rows in view are drawn, not 1,800: ${String(seen.manyDrawn)}`);
  assert.equal(seen.tabStops, 1, "one row takes Tab");
  assert.equal(seen.down, "src/many-0.ts:1", "down goes to the file's first line");
  assert.equal(seen.end, "src/many-299.ts:5", "End reaches the last row, drawn once there");
  assert.ok(Number(seen.endDrawn) < 100, String(seen.endDrawn));
  assert.equal(seen.left, "src/many-299.ts", "left on a line goes up to its file");
  assert.equal(seen.home, "src/many-0.ts");
  assert.equal(seen.collapse, "false", "left on an open file closes it");
  assert.equal(seen.expand, "true", "right on a closed file opens it");
  assert.equal(seen.intoLines, "src/many-0.ts:1", "right on an open file goes to its first line");
  assert.equal(seen.collapsedRows, true, "Collapse all leaves the files");
  assert.equal(seen.collapsedSummary, "1,500 results in 300 files");
  assert.equal(seen.manyConfirm, "Replace 1,500 matches in 300 files?");
  assert.equal(seen.manyReplaced, 300, "Replace all takes every file, drawn or not");
});
