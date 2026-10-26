import type { Unstable_DirectiveFormatter, Unstable_DirectiveSegment } from "@assistant-ui/react";
import { $createDirectiveNodeWithFormatter, DirectiveNode } from "@assistant-ui/react-lexical";
import { $isCodeNode, CodeHighlightNode, CodeNode } from "@lexical/code";
import { LinkNode } from "@lexical/link";
import { ListItemNode, ListNode } from "@lexical/list";
import {
  $convertFromMarkdownString,
  $convertToMarkdownString,
  BOLD_ITALIC_STAR,
  BOLD_ITALIC_UNDERSCORE,
  BOLD_STAR,
  BOLD_UNDERSCORE,
  CHECK_LIST,
  CODE,
  HEADING,
  INLINE_CODE,
  ITALIC_STAR,
  ITALIC_UNDERSCORE,
  LINK,
  ORDERED_LIST,
  QUOTE,
  STRIKETHROUGH,
  type TextMatchTransformer,
  type Transformer,
  UNORDERED_LIST,
} from "@lexical/markdown";
import { HorizontalRuleNode } from "@lexical/react/LexicalHorizontalRuleNode";
import { HeadingNode, QuoteNode } from "@lexical/rich-text";
import { $dfs } from "@lexical/utils";
import {
  $createTextNode,
  $isElementNode,
  $isLineBreakNode,
  $isTextNode,
  type ElementNode,
  type Klass,
  type LexicalNode,
  type TextNode,
} from "lexical";

import { IMAGE_CHIP, PASTE_CHIP } from "@/components/conversation/elements/chipText";
import { DIVIDER } from "@/components/views/page-editor/pageMarkdown";

/**
 * The rich composer's text is the message's Markdown: what it shows as formatting it sends as
 * Markdown, and Markdown put back in it (a draft, a recalled prompt, a pulled queue message)
 * shows as formatting again. The Pages editor's transformers, without tables or underline
 * (Markdown has none), and with the composer's chips kept whole through the round trip.
 */

/** Every node the rich composer can hold. */
export const COMPOSER_NODES: Klass<LexicalNode>[] = [
  HeadingNode, QuoteNode, ListNode, ListItemNode, CodeNode, CodeHighlightNode, LinkNode,
  HorizontalRuleNode, DirectiveNode,
];

/** Whether `node` is code: in a code block, or marked as inline code. */
function $inCode(node: TextNode): boolean {
  return node.hasFormat("code") || $isCodeNode(node.getParent());
}

/**
 * Whether `node` is where a line of its block starts (where `#`, `-` or `>` would start a
 * block): first in it, or after a line break.
 */
function $startsBlock(node: TextNode): boolean {
  let child: LexicalNode = node;
  for (let parent = node.getParent(); parent; child = parent, parent = parent.getParent()) {
    const previous = child.getPreviousSibling();
    if (previous !== null) return $isLineBreakNode(previous);
    if (!parent.isInline()) return true;
  }
  return true;
}

/** Text that would start a block if a line began with it. */
const BLOCK_START = /^(?:#{1,6}(?:\s|$)|[-*+](?:\s|$)|\d{1,9}[.)](?:\s|$)|>|```|(?:---|\*\*\*|___)\s*$|\[[ xX]?\]\s)/;

/** Inline markers that would format text if read back as Markdown, and the characters to escape. */
const INLINE_MARKERS: [RegExp, string][] = [
  [/`[^`]*`/, "`"],
  [/\*(?=\S)[^*]*?\S\*|\*\*(?=\S).*?\S\*\*/, "*"],
  [/(?<![\p{L}\p{N}])_(?=\S).*?\S_(?![\p{L}\p{N}])/u, "_"],
  [/~~?(?=\S).*?\S~~?/, "~"],
];

/** Where a Markdown link's text ends and its address starts, which a `\(` keeps literal. */
const LINK_TEXT = /(?<=\[[^\]]*\])\(/g;

/**
 * Stand-ins for the emphasis and code markers Lexical would escape: the export puts the
 * characters back after it, so text that would read back unchanged stays as typed.
 */
const KEPT: Record<string, string> = { "*": "\uf8f0", _: "\uf8f1", "~": "\uf8f2", "`": "\uf8f3" };
const KEPT_BACK = new Map(Object.entries(KEPT).map(([char, held]) => [held, char]));

/**
 * `text` for Lexical's export, which escapes every `*`, `_`, `~` and `` ` `` in text: a
 * backslash before punctuation doubled, and only the markers that would turn the text into
 * formatting when read back left to be escaped (with a link's `(` and a line's block marker).
 * So `snake_case` and `2 * 3` go as typed, and `**x**` typed as text as `\*\*x\*\*`.
 */
export function escapeText(text: string, startsBlock: boolean, line: string = text): string {
  let escaped = text.replace(/\\(?=[!-/:-@[-`{-~])/g, "\\\\");
  // The markers that would format it are left for Lexical to escape; the others are kept. A
  // backtick pairs with one anywhere on its line (`line`), across a chip: `` a `[Pasted text #1] ` ``.
  const escape = new Set(
    INLINE_MARKERS.flatMap(([marker, char]) => (marker.test(char === "`" ? line : escaped) ? [char] : [])),
  );
  escaped = escaped.replace(LINK_TEXT, "\\(");
  let lead = "";
  if (startsBlock && BLOCK_START.test(escaped)) {
    const number = /^\d+(?=[.)])/.exec(escaped);
    if (number) {
      lead = `${number[0]}\\`;
      escaped = escaped.slice(number[0].length);
    } else if (/^[#+>[-]/.test(escaped)) {
      lead = "\\";
    } else {
      // `*`, `_` or a backtick: left for Lexical to escape.
      lead = escaped[0] ?? "";
      escaped = escaped.slice(1);
    }
  }
  return lead + escaped.replace(/[*_~`]/g, (char) => (escape.has(char) ? char : (KEPT[char] ?? char)));
}

/**
 * Plain `text` as Markdown that reads back as that text, a paragraph per line: how pasted
 * plain text opens in the Pages editor.
 */
export function literalMarkdown(text: string): string {
  return text
    .split("\n")
    .map((line) =>
      escapeText(line, true)
        .replace(/[*_~`]/g, "\\$&")
        .replace(/[\uf8f0-\uf8f3]/gu, (held) => KEPT_BACK.get(held) ?? held),
    )
    .join("\n\n");
}

/** Stands for the caret in the Markdown while its place there is measured. */
export const CARET = "\uf8ff";

/** Where the caret is while the Markdown is measured: a text node and an offset in it. */
let caretAt: { key: string; offset: number } | null = null;

/** Each block's text while the Markdown is exported, by its key (see `$lineOf`). */
const lines = new Map<string, string>();

/** `parent`'s text in Markdown: its chips as their text, inline code as a pair of backticks. */
function inlineText(parent: ElementNode): string {
  return parent.getChildren().map((child: LexicalNode) => {
    if ($isTextNode(child)) return child.hasFormat("code") ? "``" : child.getTextContent();
    if ($isLineBreakNode(child)) return "\n";
    if ($isElementNode(child)) return child.isInline() ? inlineText(child) : "";
    return child.getTextContent();
  }).join("");
}

/**
 * The text of `node`'s block as its backticks read in Markdown: its chips as their text and its
 * inline code as a pair of backticks (a nested block's text aside).
 */
function $lineOf(node: TextNode): string {
  let block: ElementNode | null = node.getParent();
  while (block?.isInline()) block = block.getParent();
  if (!block) return node.getTextContent();
  const known = lines.get(block.getKey());
  if (known !== undefined) return known;
  const line = inlineText(block);
  lines.set(block.getKey(), line);
  return line;
}

/** Plain text exports escaped, so what was typed as text never comes back as formatting. */
const LITERAL_TEXT: TextMatchTransformer = {
  dependencies: [],
  export: (node, _children, exportFormat) => {
    if (!$isTextNode(node) || $inCode(node)) return null;
    let text = node.getTextContent();
    if (caretAt?.key === node.getKey()) text = text.slice(0, caretAt.offset) + CARET + text.slice(caretAt.offset);
    return exportFormat(node, escapeText(text, $startsBlock(node), $lineOf(node)));
  },
  importRegExp: /(?!)/,
  regExp: /(?!)/,
  replace: () => undefined,
  type: "text-match",
};

/** Order matters: a checklist item before a bulleted one, the longer emphasis markers first. */
export const COMPOSER_TRANSFORMERS: Transformer[] = [
  HEADING, QUOTE, CHECK_LIST, UNORDERED_LIST, ORDERED_LIST, DIVIDER, CODE,
  INLINE_CODE, BOLD_ITALIC_STAR, BOLD_ITALIC_UNDERSCORE, BOLD_STAR, BOLD_UNDERSCORE,
  ITALIC_STAR, ITALIC_UNDERSCORE, STRIKETHROUGH, LINK,
];

const EXPORT_TRANSFORMERS: Transformer[] = [...COMPOSER_TRANSFORMERS, LITERAL_TEXT];

/**
 * Each chip stands as a private-use character while the Markdown is read: the first is HOLD,
 * up to HOLD_LIMIT chips (more stay their text).
 */
const HOLD = 0xe000;
const HOLD_LIMIT = 0x1000;
const HOLDS = /[\ue000-\uefff]/u;
const HELD = /([\ue000-\uefff])/gu;
/** Every private-use character: none comes in from outside. */
const PRIVATE = /[\ue000-\uf8ff]/gu;

/**
 * Replaces the composer's content with `markdown`, its chips (as `parse` finds them) as chips.
 * A chip's text is held out of the Markdown while it is read, so a mention named `*draft*`
 * stays that mention; in code it is its text again. Returns whether a pasted text or image came
 * out of inline code, where the Markdown no longer says what the field shows. Call inside an
 * update.
 */
export function $importComposer(
  markdown: string,
  parse: (text: string) => readonly Unstable_DirectiveSegment[],
  formatter: Unstable_DirectiveFormatter,
): boolean {
  let lifted = false;
  const chips: Extract<Unstable_DirectiveSegment, { kind: "mention" }>[] = [];
  let held = "";
  for (const segment of parse(markdown.replace(PRIVATE, ""))) {
    if (segment.kind === "text" || chips.length >= HOLD_LIMIT) {
      held += segment.kind === "text" ? segment.text : formatter.serialize(segment);
      continue;
    }
    held += String.fromCharCode(HOLD + chips.length);
    chips.push(segment);
  }
  $convertFromMarkdownString(held, COMPOSER_TRANSFORMERS, undefined, true);
  for (const { node } of $dfs()) {
    if (!$isTextNode(node)) continue;
    const block = $isCodeNode(node.getParent());
    const code = block || node.hasFormat("code");
    const chipOf = (char: string) => chips[char.charCodeAt(0) - HOLD];
    // In code a chip is its text again, but a pasted text or image (whose text says nothing of
    // it) comes out of inline code as itself: so the composer once left backticks bare around
    // one, in drafts and messages still read back.
    const text = node.getTextContent().replace(HELD, (char) => {
      const chip = chipOf(char);
      if (!chip) return "";
      return code && (block || !PASTED_CHIPS.has(chip.type)) ? formatter.serialize(chip) : char;
    });
    if (!HOLDS.test(text)) {
      if (text !== node.getTextContent()) node.setTextContent(text);
      continue;
    }
    lifted ||= code;
    // Split at each held chip: text before it stays this node's (with its format), the chip
    // goes after it.
    const pieces = text.split(HELD);
    let last: LexicalNode = node;
    for (let index = 1; index < pieces.length; index += 2) {
      const directive = $createDirectiveNodeWithFormatter(chipOf(pieces[index]!)!, formatter);
      last.insertAfter(directive);
      last = directive;
      const rest = pieces[index + 1];
      if (rest) {
        const after = $copyText(node, rest);
        $plainIfBlank(after);
        last.insertAfter(after);
        last = after;
      }
    }
    node.setTextContent(pieces[0] ?? "");
    $plainIfBlank(node);
    if (node.getTextContent() === "") node.remove();
  }
  return lifted;
}

/** Chips that stand for something pasted, never for their own text: kept chips even in inline code. */
const PASTED_CHIPS = new Set([IMAGE_CHIP, PASTE_CHIP]);

/** Inline code left blank beside a chip taken out of it is plain text: a space, not a code span. */
function $plainIfBlank(node: TextNode): void {
  if (node.hasFormat("code") && node.getTextContent().trim() === "") node.toggleFormat("code");
}

/** A new text node with `text` and `from`'s format and style. */
function $copyText(from: TextNode, text: string): TextNode {
  const node = $createTextNode(text);
  node.setFormat(from.getFormat());
  node.setStyle(from.getStyle());
  return node;
}

/**
 * The composer's content as Markdown. With `caret` (a text node outside code, and an offset in
 * it), `CARET` marks that place in it. Call inside a read or an update.
 */
export function $exportComposer(caret: { key: string; offset: number } | null = null): string {
  caretAt = caret;
  try {
    return $convertToMarkdownString(EXPORT_TRANSFORMERS, undefined, true).replace(
      /[\uf8f0-\uf8f3]/gu,
      (held) => KEPT_BACK.get(held) ?? held,
    );
  } finally {
    caretAt = null;
    lines.clear();
  }
}
