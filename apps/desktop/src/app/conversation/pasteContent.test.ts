import assert from "node:assert/strict";
import { test } from "node:test";

import { createHeadlessEditor } from "@lexical/headless";
import { $getRoot } from "lexical";

import { literalMarkdown } from "@/app/conversation/composerMarkdown";
import { $importPage, PAGE_NODES } from "@/app/conversation/pages/pageMarkdown";

import { PASTE_CHIP_CHARS, pasteKind, type PastedText } from "./pasteContent";

const paste = (over: Partial<PastedText>): PastedText => ({ text: "", html: "", own: false, code: false, ...over });
const structured = (html: string) => html.includes("<li>");

test("what a paste becomes: rich text with more than a phrase and long text a chip, a phrase formatted, short text as typed", () => {
  assert.equal(pasteKind(paste({ text: "a\nb", html: "<ul><li>a</li><li>b</li></ul>" }), structured), "chip");
  assert.equal(pasteKind(paste({ text: "bold words", html: "<b>bold</b> words" }), structured), "inline");
  assert.equal(pasteKind(paste({ text: "x".repeat(PASTE_CHIP_CHARS) }), structured), "chip");
  assert.equal(pasteKind(paste({ text: "x".repeat(PASTE_CHIP_CHARS - 1) }), structured), "literal");
  assert.equal(pasteKind(paste({ text: "https://a.b/c" }), structured), "literal");
  // The composer's own copy is the field's to paste; a code editor's colours are not rich text.
  assert.equal(pasteKind(paste({ text: "a", html: "<ul><li>a</li></ul>", own: true }), structured), "own");
  assert.equal(pasteKind(paste({ text: "let x", html: "<ul><li>x</li></ul>", code: true }), structured), "literal");
  assert.equal(PASTE_CHIP_CHARS, 2000);
});

test("pasted plain text opens in the Pages editor as its own lines, literally", () => {
  const text = "# not a heading\n**not bold** and snake_case\n- not a list";
  const editor = createHeadlessEditor({ nodes: PAGE_NODES, onError: (error) => { throw error; } });
  editor.update(() => $importPage(literalMarkdown(text)), { discrete: true });
  editor.getEditorState().read(() => {
    const blocks = $getRoot().getChildren();
    assert.deepEqual(blocks.map((block) => block.getType()), ["paragraph", "paragraph", "paragraph"]);
    assert.deepEqual(blocks.map((block) => block.getTextContent()), text.split("\n"));
  });
});
