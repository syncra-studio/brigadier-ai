import assert from "node:assert/strict";
import { test } from "node:test";

import { createHeadlessEditor } from "@lexical/headless";
import { $isListNode, type ListItemNode, type ListNode } from "@lexical/list";
import { $isHeadingNode } from "@lexical/rich-text";
import { $getRoot, type ElementNode } from "lexical";

import { $exportPage, $importPage, PAGE_NODES } from "./pageMarkdown";

function roundTrip(markdown: string): string {
  const editor = createHeadlessEditor({ nodes: PAGE_NODES, onError: (error) => { throw error; } });
  editor.update(() => $importPage(markdown), { discrete: true });
  return editor.getEditorState().read(() => $exportPage());
}

const EVERY_FEATURE = [
  "# Heading one",
  "## Heading two",
  "### Heading three",
  "Plain with **bold**, *italic*, <u>underline</u>, ~~strike~~, `code` and a [link](https://example.com).",
  "- bulleted\n- list",
  "1. numbered\n2. list",
  "- [ ] open task\n- [x] done task",
  "> a quote",
  "```\nconst a = 1;\n```",
  "| Name | Value |\n| --- | --- |\n| **a** | 1 |\n| b \\| c | 2 |",
  "---",
  "Last paragraph",
].join("\n\n");

test("every block and style a page offers comes back unchanged", () => {
  assert.equal(roundTrip(EVERY_FEATURE), EVERY_FEATURE);
});

test("a second round trip changes nothing", () => {
  const once = roundTrip("Intro\n\n* star bullet\n\n__bold__ _it_\n\n***\n\n|a|b|\n|-|-|\n|1|2|");
  assert.equal(roundTrip(once), once);
});

test("an empty page stays empty", () => {
  assert.equal(roundTrip(""), "");
});

test("each Markdown block becomes its own editor block, not a paragraph of its text", () => {
  const editor = createHeadlessEditor({ nodes: PAGE_NODES, onError: (error) => { throw error; } });
  editor.update(() => $importPage(EVERY_FEATURE), { discrete: true });
  const shape = editor.getEditorState().read(() => $getRoot().getChildren().map((node) =>
    $isListNode(node) ? `list:${node.getListType()}` : $isHeadingNode(node) ? node.getTag() : node.getType()));
  assert.deepEqual(shape, ["h1", "h2", "h3", "paragraph", "list:bullet", "list:number", "list:check", "quote", "code", "table", "horizontalrule", "paragraph"]);
  const formats = editor.getEditorState().read(() => $getRoot().getChildAtIndex<ElementNode>(3)!.getAllTextNodes()
    .flatMap((node) => (["bold", "italic", "underline", "strikethrough", "code"] as const).filter((format) => node.hasFormat(format))));
  assert.deepEqual(formats, ["bold", "italic", "underline", "strikethrough", "code"]);
  const checked = editor.getEditorState().read(() => $getRoot().getChildAtIndex<ListNode>(6)!.getChildren<ListItemNode>().map((item) => item.getChecked()));
  assert.deepEqual(checked, [false, true]);
});
