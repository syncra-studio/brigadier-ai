import { $createCodeNode, $isCodeNode } from "@lexical/code";
import { $isListNode, INSERT_CHECK_LIST_COMMAND, INSERT_ORDERED_LIST_COMMAND, INSERT_UNORDERED_LIST_COMMAND, ListNode, REMOVE_LIST_COMMAND } from "@lexical/list";
import { INSERT_HORIZONTAL_RULE_COMMAND } from "@lexical/react/LexicalHorizontalRuleNode";
import { $createHeadingNode, $createQuoteNode, $isHeadingNode, $isQuoteNode } from "@lexical/rich-text";
import { $setBlocksType } from "@lexical/selection";
import { INSERT_TABLE_COMMAND } from "@lexical/table";
import { $findMatchingParent, $getNearestNodeOfType } from "@lexical/utils";
import {
  $createParagraphNode,
  $getRoot,
  $getSelection,
  $isRangeSelection,
  $isRootOrShadowRoot,
  type ElementNode,
  type LexicalEditor,
  type LexicalNode,
} from "lexical";
import type { ReactNode } from "react";

/** Every kind of block a page offers, as the Insert menu, `/` and the Text picker list them. */
export type BlockKind = "paragraph" | "h1" | "h2" | "h3" | "bullet" | "number" | "check" | "quote" | "code" | "table" | "divider";

export type Block = {
  kind: BlockKind;
  label: string;
  /** The Markdown typed at a line's start that makes it, shown right-aligned. */
  markdown: string | null;
  icon: ReactNode;
  /** Other words `/` finds it by. */
  words: string;
};

const svg = (children: ReactNode) => (
  <svg aria-hidden viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeLinecap="round" strokeLinejoin="round" className="page-block-icon size-icon-sm shrink-0">{children}</svg>
);
const letters = (text: string) => svg(<text x="8" y="11.5" textAnchor="middle" fill="currentColor" stroke="none" fontSize="8.5" fontWeight="600">{text}</text>);

export const BLOCKS: Block[] = [
  { kind: "paragraph", label: "Text", markdown: null, words: "paragraph plain", icon: letters("Aa") },
  { kind: "h1", label: "Heading 1", markdown: "#", words: "title big", icon: letters("H1") },
  { kind: "h2", label: "Heading 2", markdown: "##", words: "subtitle", icon: letters("H2") },
  { kind: "h3", label: "Heading 3", markdown: "###", words: "small", icon: letters("H3") },
  { kind: "bullet", label: "Bulleted list", markdown: "-", words: "unordered bullets", icon: svg(<><circle cx="3" cy="4.5" r=".9" fill="currentColor" /><circle cx="3" cy="11.5" r=".9" fill="currentColor" /><path d="M6.5 4.5h7M6.5 11.5h7" /></>) },
  { kind: "number", label: "Numbered list", markdown: "1.", words: "ordered numbers", icon: svg(<><path d="M2.5 3.2l1-.7v4M2.3 13.5h2.2l-2.2-2.4c.6-1 2.2-.8 2.2.3" /><path d="M7 4.5h6.5M7 11.5h6.5" /></>) },
  { kind: "check", label: "Checklist", markdown: "[]", words: "todo tasks checkbox", icon: svg(<><path d="M4.5 2.5h6A2.5 2.5 0 0 1 13 5v6a2.5 2.5 0 0 1-2.5 2.5h-6A2.5 2.5 0 0 1 2 11V5a2.5 2.5 0 0 1 2.5-2.5z" /><path d="M5 8l2 2 3.5-4" /></>) },
  { kind: "quote", label: "Quote", markdown: ">", words: "blockquote", icon: svg(<><path d="M3 3.5v9" /><path d="M6.5 5h7M6.5 8h7M6.5 11h4.5" /></>) },
  { kind: "code", label: "Code", markdown: "```", words: "snippet monospace", icon: svg(<path d="M5.5 4.5L2 8l3.5 3.5M10.5 4.5L14 8l-3.5 3.5" />) },
  { kind: "table", label: "Table", markdown: null, words: "grid rows columns", icon: svg(<><path d="M4 2.5h8a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2z" /><path d="M2 6.5h12M6.5 6.5v7" /></>) },
  { kind: "divider", label: "Divider", markdown: "---", words: "rule separator line hr", icon: svg(<path d="M2 8h12" />) },
];

/** The kinds the Text picker turns a block into; tables and dividers are inserted, not converted. */
export const TURN_INTO = BLOCKS.filter((block) => block.kind !== "table" && block.kind !== "divider");

/** The block the caret is in. Call inside a read. */
export function $currentBlock(): BlockKind | null {
  const selection = $getSelection();
  if (!$isRangeSelection(selection)) return null;
  const anchor = selection.anchor.getNode();
  const top: LexicalNode | null = anchor.getKey() === "root" ? null
    : $findMatchingParent(anchor, (node) => { const parent = node.getParent(); return parent !== null && $isRootOrShadowRoot(parent); });
  if (!top) return null;
  if ($isListNode(top)) {
    const list = $getNearestNodeOfType<ListNode>(anchor, ListNode);
    const type = (list ?? top).getListType();
    return type === "bullet" ? "bullet" : type === "number" ? "number" : "check";
  }
  if ($isHeadingNode(top)) { const tag = top.getTag(); return tag === "h1" || tag === "h2" || tag === "h3" ? tag : "h3"; }
  if ($isQuoteNode(top)) return "quote";
  if ($isCodeNode(top)) return "code";
  return "paragraph";
}

/** Gives the block a place to go when the caret isn't in the page: a new last line. */
function $ensureCaret(): void {
  if ($isRangeSelection($getSelection())) return;
  const root = $getRoot();
  const last = root.getLastChild();
  if (last && last.getType() === "paragraph" && last.getTextContentSize() === 0) (last as ElementNode).selectEnd();
  else { const line = $createParagraphNode(); root.append(line); line.select(); }
}

/** Turns the caret's block into `kind`, or inserts it there (a table, a divider). */
export function applyBlock(editor: LexicalEditor, kind: BlockKind): void {
  editor.update(() => {
    $ensureCaret();
    const selection = $getSelection();
    if (!$isRangeSelection(selection)) return;
    const current = $currentBlock();
    const list = { bullet: INSERT_UNORDERED_LIST_COMMAND, number: INSERT_ORDERED_LIST_COMMAND, check: INSERT_CHECK_LIST_COMMAND } as const;
    if (kind === "bullet" || kind === "number" || kind === "check") {
      editor.dispatchCommand(current === kind ? REMOVE_LIST_COMMAND : list[kind], undefined);
      return;
    }
    if (kind === "table" || kind === "divider") {
      // Goes after the caret's block on a line of its own, leaving that block whole.
      const top = selection.anchor.getNode().getTopLevelElement();
      let line: ElementNode | null = top && top.getType() === "paragraph" && top.getTextContentSize() === 0 ? top : null;
      if (top && !line) {
        line = $createParagraphNode();
        top.insertAfter(line);
        line.select();
      }
      if (kind === "table") editor.dispatchCommand(INSERT_TABLE_COMMAND, { rows: "3", columns: "3", includeHeaders: { rows: true, columns: false } });
      else editor.dispatchCommand(INSERT_HORIZONTAL_RULE_COMMAND, undefined);
      // The empty line it went into isn't kept above it.
      if (line?.isAttached() && line.getTextContentSize() === 0 && line.getNextSibling()?.getType() !== "paragraph") line.remove();
      return;
    }
    if (current === "bullet" || current === "number" || current === "check") editor.dispatchCommand(REMOVE_LIST_COMMAND, undefined);
    const next = $getSelection();
    if (!$isRangeSelection(next)) return;
    $setBlocksType(next, () =>
      kind === "h1" || kind === "h2" || kind === "h3" ? $createHeadingNode(kind)
        : kind === "quote" ? $createQuoteNode()
        : kind === "code" ? $createCodeNode()
        : $createParagraphNode());
  });
}

/** Whether `block` matches what was typed after `/`. */
export function blockMatches(block: Block, query: string): boolean {
  const words = `${block.label} ${block.words}`.toLowerCase();
  return query.toLowerCase().split(/\s+/).every((word) => words.includes(word));
}
