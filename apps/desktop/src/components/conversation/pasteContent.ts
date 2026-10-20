import { createHeadlessEditor } from "@lexical/headless";
import { $generateNodesFromDOM } from "@lexical/html";
import { $getRoot, $insertNodes } from "lexical";

import { $exportPage, PAGE_NODES } from "@/components/views/page-editor/pageMarkdown";

/**
 * What a paste of text into the composer becomes. Rich text copied from a page or a document
 * (more than one line or block of it), and plain text of PASTE_CHIP_CHARS or more, is
 * pasted as a chip: `[Pasted text #n]` where it was pasted, its content (as Markdown) sent
 * there. A phrase copied with its formatting or link goes in as text, formatted; other text
 * goes in as typed.
 */
/** Plain text this long, pasted, becomes a "Pasted text" chip (or card) instead of text. */
export const PASTE_CHIP_CHARS = 2000;

export type PasteKind =
  /** The composer's own copy, which the field pastes itself. */
  | "own"
  /** A chip of the pasted content (or a card, into an empty composer). */
  | "chip"
  /** Text, its inline formatting and links kept where the field has formatting. */
  | "inline"
  /** Text as typed. */
  | "literal";

/** The clipboard as a paste reads it. */
export type PastedText = {
  text: string;
  html: string;
  /** The clipboard is a copy from this composer's own editor (Lexical's own format). */
  own: boolean;
  /** The clipboard is a code editor's copy: its HTML is only colours, the text is the code. */
  code: boolean;
};

/** What `paste` becomes; `structured` says whether its HTML has more than one line or block. */
export function pasteKind(paste: PastedText, structured: (html: string) => boolean): PasteKind {
  if (paste.own) return "own";
  const html = paste.code ? "" : paste.html;
  if (paste.text.length >= PASTE_CHIP_CHARS) return "chip";
  if (html.trim() === "") return "literal";
  return structured(html) ? "chip" : "inline";
}

/** The clipboard's text, as a paste reads it. */
export function pastedText(clipboard: DataTransfer, namespace: string): PastedText {
  return {
    text: clipboard.getData("text/plain"),
    html: clipboard.getData("text/html"),
    own: ownCopy(clipboard.getData("application/x-lexical-editor"), namespace),
    code: clipboard.types.includes("vscode-editor-data"),
  };
}

function ownCopy(lexical: string, namespace: string): boolean {
  if (!lexical) return false;
  try {
    return (JSON.parse(lexical) as { namespace?: unknown }).namespace === namespace;
  } catch {
    return false;
  }
}

/** Elements that make a block of their own. */
const BLOCKS = "address,article,aside,blockquote,dd,div,dl,dt,figure,footer,h1,h2,h3,h4,h5,h6,header,hr,li,main,nav,ol,p,pre,section,table,td,th,tr,ul";
/** Elements that are structure even alone: a list, a heading, a table, code, a quote, a rule, a picture. */
const STRUCTURE = "blockquote,dl,h1,h2,h3,h4,h5,h6,hr,img,ol,pre,table,ul";

/**
 * Whether copied HTML is more than a phrase: a list, heading, table, code block, quote, rule or
 * picture, more than one block with text, or a line break in its text. One bold phrase or one
 * link (even wrapped in a paragraph, as a document copies it) isn't.
 */
export function structuredHtml(html: string): boolean {
  const body = new DOMParser().parseFromString(html, "text/html").body;
  if (body.querySelector(STRUCTURE)) return true;
  // A browser ends a copy that takes a line's end with a break of its own.
  const breaks = [...body.querySelectorAll("br:not(.Apple-interchange-newline)")];
  if (breaks.some((br) => br.nextSibling || br.previousSibling)) return true;
  const leaves = [...body.querySelectorAll(BLOCKS)].filter(
    (block) => !block.querySelector(BLOCKS) && (block.textContent ?? "").trim() !== "",
  );
  return leaves.length > 1;
}

/** Copied HTML as Markdown, as the Pages editor reads it (its tables, lists, code and all). */
export function htmlToMarkdown(html: string): string {
  const editor = createHeadlessEditor({
    nodes: PAGE_NODES,
    onError: (error) => {
      throw error;
    },
  });
  editor.update(
    () => {
      const dom = new DOMParser().parseFromString(html, "text/html");
      const nodes = $generateNodesFromDOM(editor, dom);
      $getRoot().clear().select();
      $insertNodes(nodes);
    },
    { discrete: true },
  );
  return editor.getEditorState().read(() => $exportPage()).trim();
}

/** What a chip of `paste` holds: its Markdown (rich text) or its text, and which. */
export function chipContent(paste: PastedText): { content: string; markdown: boolean } {
  const html = paste.code ? "" : paste.html;
  if (html.trim() !== "") {
    const markdown = htmlToMarkdown(html);
    if (markdown !== "") return { content: markdown, markdown: true };
  }
  return { content: paste.text, markdown: false };
}
