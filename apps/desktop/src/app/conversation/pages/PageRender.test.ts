import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

const INSERT = ["H1Heading 1#", "H2Heading 2##", "H3Heading 3###", "Bulleted list-", "Numbered list1.", "Checklist[]", "Quote>", "Code```", "Table", "Divider---"];

test("a page: shortcuts, the Insert list, formatting, dragging, autosave, conflicts and the File menu", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "page.html?drive=1", "page-result", 90000)) as Record<string, unknown>;
  if (process.env.PAGE_SEEN) console.log(JSON.stringify(seen, null, 2));
  assert.equal(seen.error, undefined);
  // A new page starts in its "Untitled page" title; Enter goes on to the body.
  assert.equal(seen.newPageStartsInTitle, true);
  assert.equal(seen.placeholder, "Untitled page");
  assert.equal(seen.enterMovesToBody, true);
  assert.deepEqual(seen.measure, { bar: { width: 1280, height: 48 }, column: { width: 736, height: (seen.measure as { column: { height: number } }).column.height },
    title: [28, 600, 33.6], body: [14, 22.75] });
  // Each Markdown shortcut makes its block.
  assert.deepEqual(seen.markdownBlocks, ["h1", "h2", "h3", "ul", "ol", "ul.check", "blockquote", "p", "hr", "p"]);
  // `/` and the bar's Insert list the same blocks with their Markdown; typing narrows `/`.
  assert.deepEqual(seen.slashItems, INSERT);
  assert.equal(seen.slashMenuWidth, 360);
  assert.deepEqual(seen.slashFiltered, ["Code```"]);
  assert.equal(seen.codeBlock, "code");
  assert.deepEqual(seen.insertItems, INSERT);
  assert.deepEqual(seen.table, [3, 3]);
  // Selected text gets the toolbar; Bold formats it.
  assert.deepEqual(seen.toolbar, ["BBold", "IItalic", "UUnderline", "SStrikethrough", "Code", "Link", "Turn into, now Heading 1"]);
  assert.equal(seen.boldApplied, true);
  // The handle shows in the gutter beside the block under the pointer and moves it.
  assert.equal(seen.handleShows, true);
  assert.equal(seen.handleBesideBlock, true);
  assert.equal(seen.dropLineShows, true);
  assert.deepEqual(seen.afterDrag, ["h1", "h3", "ul", "ol", "h2", "ul.check", "blockquote", "p", "hr", "code", "table", "p"]);
  // Saved as Markdown once typing stops.
  assert.deepEqual(seen.saved, {
    title: "Launch plan", emoji: null,
    body: "# **One**\n\n### Three\n\n- bullet\n\n1. first\n\n## Two\n\n- [ ] task\n\n> quoted\n\n---\n\n```\nlet x = 1;\n```\n\n|  |  |  |\n| --- | --- | --- |\n|  |  |  |\n|  |  |  |\n",
  });
  // A change from elsewhere with unsaved edits asks; Keep both keeps each side.
  assert.deepEqual(seen.conflict, ["This page changed elsewhere. Which version do you want to keep?", "Keep mine", "Use theirs", "Keep both"]);
  const kept = seen.keptBoth as { mine: string; title: string; theirs: string };
  assert.ok(kept.mine.endsWith("!"), "my unsaved edit is in the copy");
  assert.equal(kept.title, "Launch plan (copy)");
  assert.equal(kept.theirs, "Their version");
  assert.equal(seen.editorShowsTheirs, "Their version");
  // The File menu.
  assert.deepEqual(seen.refusedWhileConflict, { problem: "Not sent. This page changed elsewhere. Choose which version to keep first.", kept: "tidy this up", sent: 0 });
  assert.deepEqual(seen.sentFromPage, {
    text: "tick the first item", mentions: [{ type: "page", id: "1".repeat(32), title: "Launch plan" }], afterWrite: true,
    cleared: true, placeholder: "Work with this page…",
  });
  assert.deepEqual(seen.fileItems, ["Show pages⇧⌘E", "Rename", "Copy as Markdown", "Download", "Delete"]);
  assert.deepEqual(seen.typedWhileSaving, { sent: "add a date", kept: "add a date and a time" });
  assert.deepEqual(seen.modEnter, { enterSent: false, modEnterSent: "one more" });
  assert.equal(seen.copied, "# Launch plan\n\nTheir version before sending!");
  assert.equal(seen.renameFocus, true);
  assert.deepEqual(seen.renameSelection, [0, 11, 11]);
  assert.deepEqual(seen.download, { name: "Launch plan.md", choose: true, sameText: true });
  assert.deepEqual(seen.pageList, ["Launch plan", "📌Older notes"]);
  assert.deepEqual(seen.openedFromList, ["1".repeat(32), "2".repeat(32)]);
  assert.equal(seen.deleteAsks, "Delete this page?");
  assert.equal(seen.deleted, true);
  assert.deepEqual(seen.tabsAfterDelete, ["2".repeat(32)]);
});
