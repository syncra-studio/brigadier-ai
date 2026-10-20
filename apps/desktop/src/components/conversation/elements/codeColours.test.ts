import assert from "node:assert/strict";
import { test } from "node:test";

import { $createCodeHighlightNode, $createCodeNode, CodeHighlightNode, CodeNode } from "@lexical/code";
import { createHeadlessEditor } from "@lexical/headless";
import { $createRangeSelection, $getRoot, $getSelection, $setSelection, type TextNode } from "lexical";

import { $applyColours } from "./codeColours";

const token = (content: string, color?: string) => ({ content, color, italic: false });

test("colouring a code block keeps a selection in it, both its ends", () => {
  const field = createHeadlessEditor({ nodes: [CodeNode, CodeHighlightNode], onError: (error) => { throw error; } });
  let key = "";
  field.update(
    () => {
      const text = $createCodeHighlightNode("let x = 1");
      const code = $createCodeNode("ts").append(text);
      $getRoot().append(code);
      key = code.getKey();
      const selection = $createRangeSelection();
      selection.anchor.set(text.getKey(), 4, "text");
      selection.focus.set(text.getKey(), 9, "text");
      $setSelection(selection);
    },
    { discrete: true },
  );
  field.update(() => $applyColours(key, "let x = 1", [[token("let", "var(--a)"), token(" x = "), token("1", "var(--b)")]]), { discrete: true });
  field.getEditorState().read(() => {
    assert.equal(($getRoot().getFirstChild() as CodeNode).getChildrenSize(), 3);
    const selection = $getSelection();
    assert.equal(selection?.getTextContent(), "x = 1");
    assert.equal((selection!.getNodes().at(-1) as TextNode).getStyle(), "color: var(--b);");
  });
});
