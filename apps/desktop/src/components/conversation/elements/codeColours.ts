import { $createCodeHighlightNode, $isCodeHighlightNode, $isCodeNode, type CodeNode } from "@lexical/code";
import { $createLineBreakNode, $createTabNode, $getNodeByKey, $getSelection, $isLineBreakNode, $isRangeSelection, $isTextNode } from "lexical";

/** A coloured piece of a code line, as the highlighter gives it. */
export type Token = { content: string; color: string | undefined; italic: boolean };

/** The style a token's text gets. */
function tokenStyle(token: Token): string {
  return `${token.color ? `color: ${token.color};` : ""}${token.italic ? " font-style: italic;" : ""}`.trim();
}

/** Colours code block `key` with `lines`, if its text is still `text`; the selection stays put. */
export function $applyColours(key: string, text: string, lines: Token[][]) {
  const code = $getNodeByKey(key);
  if (!$isCodeNode(code) || code.getTextContent() !== text) return;
  const want = lines.flatMap((line, at) => [
    ...(at > 0 ? [{ content: "\n", style: "" }] : []),
    ...line.flatMap((token) =>
      token.content.split(/(\t)/).filter(Boolean).map((content) => ({ content, style: content === "\t" ? "" : tokenStyle(token) })),
    ),
  ]);
  const have = code.getChildren().map((child) => ({
    content: child.getTextContent(),
    style: $isCodeHighlightNode(child) ? child.getStyle() : "",
  }));
  if (JSON.stringify(want) === JSON.stringify(have)) return;
  // Where the selection's ends are, as offsets in the block's text.
  const selection = $getSelection();
  const ends = $isRangeSelection(selection)
    ? [selection.anchor, selection.focus].map((point) => ({
        key: point.key,
        offset: point.offset,
        type: point.type,
        at: $codeOffset(code, point),
      }))
    : [];
  code.clear();
  for (const piece of want) {
    if (piece.content === "\n") code.append($createLineBreakNode());
    else if (piece.content === "\t") code.append($createTabNode());
    else {
      const node = $createCodeHighlightNode(piece.content);
      if (piece.style) node.setStyle(piece.style);
      code.append(node);
    }
  }
  const after = $getSelection();
  if (!$isRangeSelection(after) || !ends.some((end) => end.at !== null)) return;
  [after.anchor, after.focus].forEach((point, index) => {
    const end = ends[index]!;
    if (end.at === null) point.set(end.key, end.offset, end.type);
    else point.set(...$codePoint(code, end.at));
  });
}

/** `point`'s offset in `code`'s text, or null when it is outside the block. */
function $codeOffset(code: CodeNode, point: { key: string; offset: number; type: "text" | "element" }): number | null {
  const children = code.getChildren();
  if (point.key === code.getKey()) {
    return children.slice(0, point.offset).reduce((sum, child) => sum + child.getTextContentSize(), 0);
  }
  let offset = 0;
  for (const child of children) {
    if (child.getKey() === point.key) return offset + (point.type === "text" ? point.offset : 0);
    offset += child.getTextContentSize();
  }
  return null;
}

/** The selection point at offset `at` in `code`'s text: in a text node, or before a line break. */
function $codePoint(code: CodeNode, at: number): [string, number, "text" | "element"] {
  let left = at;
  const children = code.getChildren();
  for (const [index, child] of children.entries()) {
    const size = child.getTextContentSize();
    if ($isTextNode(child) && left <= size) return [child.getKey(), left, "text"];
    if ($isLineBreakNode(child) && left === 0) return [code.getKey(), index, "element"];
    left -= size;
  }
  return [code.getKey(), children.length, "element"];
}
