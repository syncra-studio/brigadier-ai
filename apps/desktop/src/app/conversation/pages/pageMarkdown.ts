import { CodeNode } from "@lexical/code";
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
  type ElementTransformer,
  HEADING,
  INLINE_CODE,
  isTableRowDivider,
  ITALIC_STAR,
  ITALIC_UNDERSCORE,
  LINK,
  type MultilineElementTransformer,
  ORDERED_LIST,
  QUOTE,
  STRIKETHROUGH,
  type TextMatchTransformer,
  type Transformer,
  UNORDERED_LIST,
} from "@lexical/markdown";
import { $createHorizontalRuleNode, $isHorizontalRuleNode, HorizontalRuleNode } from "@lexical/react/LexicalHorizontalRuleNode";
import { HeadingNode, QuoteNode } from "@lexical/rich-text";
import {
  $createTableCellNode,
  $createTableNode,
  $createTableRowNode,
  $isTableCellNode,
  $isTableNode,
  $isTableRowNode,
  TableCellHeaderStates,
  TableCellNode,
  TableNode,
  TableRowNode,
} from "@lexical/table";
import { $isTextNode, type Klass, type LexicalNode } from "lexical";

/**
 * A page's body is plain Markdown on disk. Every block and style the editor offers has a
 * transformer here, so what is typed comes back the same after a save and a reload.
 */

/** Every node a page can hold. */
export const PAGE_NODES: Klass<LexicalNode>[] = [
  HeadingNode, QuoteNode, ListNode, ListItemNode, CodeNode, LinkNode,
  TableNode, TableRowNode, TableCellNode, HorizontalRuleNode,
];

export const DIVIDER: ElementTransformer = {
  dependencies: [HorizontalRuleNode],
  export: (node) => ($isHorizontalRuleNode(node) ? "---" : null),
  regExp: /^(?:---|\*\*\*|___)\s?$/,
  replace: (parent, _children, _match, isImport) => {
    const line = $createHorizontalRuleNode();
    if (isImport || parent.getNextSibling() !== null) parent.replace(line);
    else parent.insertBefore(line);
    line.selectNext();
  },
  type: "element",
  triggerOnEnter: true,
};

/** Markdown has no underline of its own; `<u>` is what Markdown viewers show as one. */
const UNDERLINE: TextMatchTransformer = {
  dependencies: [],
  export: (node, _children, exportFormat) =>
    $isTextNode(node) && node.hasFormat("underline") ? `<u>${exportFormat(node, node.getTextContent())}</u>` : null,
  importRegExp: /<u>(.+?)<\/u>/,
  regExp: /<u>(.+?)<\/u>$/,
  replace: (node, match) => {
    node.setTextContent(match[1] ?? "");
    if (!node.hasFormat("underline")) node.toggleFormat("underline");
    return node;
  },
  trigger: ">",
  type: "text-match",
};

const TABLE_ROW = /^\|(.*)\|\s*$/;

function cells(line: string): string[] {
  const inner = TABLE_ROW.exec(line)?.[1] ?? "";
  return inner.split(/(?<!\\)\|/).map((cell) => cell.trim().replaceAll("\\|", "|")
    .replaceAll("<br>", "\n").replaceAll("&lt;", "<").replaceAll("&gt;", ">").replaceAll("&amp;", "&"));
}

function $cell(text: string, header: boolean): TableCellNode {
  const cell = $createTableCellNode(header ? TableCellHeaderStates.ROW : TableCellHeaderStates.NO_STATUS);
  $convertFromMarkdownString(text, PAGE_TRANSFORMERS, cell);
  return cell;
}

/** A GitHub-style table: a header row, the `| --- |` line, then the rows. */
const TABLE: MultilineElementTransformer = {
  dependencies: [TableNode, TableRowNode, TableCellNode],
  export: (node) => {
    if (!$isTableNode(node)) return null;
    const rows = node.getChildren().filter($isTableRowNode).map((row) =>
      row.getChildren().filter($isTableCellNode).map((cell) =>
        $convertToMarkdownString(PAGE_TRANSFORMERS, cell)
          .replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;")
          .replaceAll("\n", "<br>").replaceAll("|", "\\|")));
    const width = Math.max(1, ...rows.map((row) => row.length));
    const line = (row: string[]) => `| ${Array.from({ length: width }, (_, at) => row[at] ?? "").join(" | ")} |`;
    const [head = [], ...body] = rows;
    return [line(head), line(Array.from({ length: width }, () => "---")), ...body.map(line)].join("\n");
  },
  regExpStart: TABLE_ROW,
  handleImportAfterStartMatch: ({ lines, rootNode, startLineIndex }) => {
    if (!isTableRowDivider(lines[startLineIndex + 1] ?? "")) return null;
    let end = startLineIndex + 1;
    while (TABLE_ROW.test(lines[end + 1] ?? "")) end += 1;
    const table = $createTableNode();
    const rows = [cells(lines[startLineIndex]!), ...lines.slice(startLineIndex + 2, end + 1).map(cells)];
    const width = Math.max(...rows.map((row) => row.length));
    rows.forEach((row, at) => {
      const tableRow = $createTableRowNode();
      for (let column = 0; column < width; column += 1) tableRow.append($cell(row[column] ?? "", at === 0));
      table.append(tableRow);
    });
    rootNode.append(table);
    return [true, end];
  },
  replace: () => false,
  type: "multiline-element",
};

/** Order matters: a checklist item before a bulleted one, the longer emphasis markers first. */
export const PAGE_TRANSFORMERS: Transformer[] = [
  HEADING, QUOTE, CHECK_LIST, UNORDERED_LIST, ORDERED_LIST, DIVIDER,
  CODE, TABLE,
  INLINE_CODE, BOLD_ITALIC_STAR, BOLD_ITALIC_UNDERSCORE, BOLD_STAR, BOLD_UNDERSCORE,
  ITALIC_STAR, ITALIC_UNDERSCORE, STRIKETHROUGH,
  UNDERLINE, LINK,
];

/** Replaces the editor's content with `markdown`. Call inside an update. */
export function $importPage(markdown: string): void {
  $convertFromMarkdownString(markdown, PAGE_TRANSFORMERS);
}

/** The editor's content as Markdown. Call inside a read or an update. */
export function $exportPage(): string {
  return $convertToMarkdownString(PAGE_TRANSFORMERS);
}
