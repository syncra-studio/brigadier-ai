import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage } from "@/fixtures/headless";

test("Search streams grouped results, opens a line, asks again on its options, replaces after a confirm and clears", { timeout: 60000 }, async (t) => {
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
});
