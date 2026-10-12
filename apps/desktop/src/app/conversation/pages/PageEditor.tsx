import { CheckListPlugin } from "@lexical/react/LexicalCheckListPlugin";
import { LexicalComposer } from "@lexical/react/LexicalComposer";
import { ContentEditable } from "@lexical/react/LexicalContentEditable";
import { EditorRefPlugin } from "@lexical/react/LexicalEditorRefPlugin";
import { LexicalErrorBoundary } from "@lexical/react/LexicalErrorBoundary";
import { HistoryPlugin } from "@lexical/react/LexicalHistoryPlugin";
import { HorizontalRulePlugin } from "@lexical/react/LexicalHorizontalRulePlugin";
import { LinkPlugin } from "@lexical/react/LexicalLinkPlugin";
import { ListPlugin } from "@lexical/react/LexicalListPlugin";
import { MarkdownShortcutPlugin } from "@lexical/react/LexicalMarkdownShortcutPlugin";
import { OnChangePlugin } from "@lexical/react/LexicalOnChangePlugin";
import { RichTextPlugin } from "@lexical/react/LexicalRichTextPlugin";
import { TabIndentationPlugin } from "@lexical/react/LexicalTabIndentationPlugin";
import { TablePlugin } from "@lexical/react/LexicalTablePlugin";
import { $getRoot, type EditorThemeClasses, type LexicalEditor } from "lexical";
import type { ReactNode, RefObject } from "react";

import { $exportPage, $importPage, PAGE_NODES, PAGE_TRANSFORMERS } from "./pageMarkdown";

const THEME: EditorThemeClasses = {
  paragraph: "page-p",
  heading: { h1: "page-h1", h2: "page-h2", h3: "page-h3", h4: "page-h4", h5: "page-h4", h6: "page-h4" },
  quote: "page-quote",
  list: {
    ul: "page-ul", ol: "page-ol", checklist: "page-checklist", listitem: "page-li",
    listitemChecked: "page-li-checked", listitemUnchecked: "page-li-unchecked",
    nested: { listitem: "page-li-nested" },
  },
  link: "page-link",
  code: "page-code",
  hr: "page-hr",
  table: "page-table",
  tableCell: "page-td",
  tableCellHeader: "page-th",
  text: {
    bold: "page-bold", italic: "page-italic", underline: "page-underline", strikethrough: "page-strike",
    underlineStrikethrough: "page-underline-strike", code: "page-inline-code",
  },
};

/** Links on a page only ever open web and mail addresses. */
export function safeLink(url: string): boolean {
  return /^(?:https?:|mailto:)/i.test(url);
}

/**
 * The page's body: Markdown in, Markdown out on every change. Remount it (a new `key`) to load
 * content that changed outside it.
 */
export function PageEditor({ body, label, editorRef, onChange, onOpenLink, children }: {
  body: string;
  label: string;
  /** ⌘-click (Ctrl-click) on a link. */
  onOpenLink: (url: string) => void;
  editorRef: RefObject<LexicalEditor | null>;
  onChange: (markdown: string) => void;
  /** Plugins that need the editor: the Insert menu, the selection toolbar, the drag handles. */
  children?: ReactNode;
}) {
  return (
    <LexicalComposer initialConfig={{
      namespace: "page",
      nodes: PAGE_NODES,
      theme: THEME,
      onError: (error) => console.error("The page editor failed", error),
      editorState: () => $importPage(body),
    }}>
      {/* oxlint-disable-next-line jsx-a11y/click-events-have-key-events, jsx-a11y/no-static-element-interactions -- the links inside are reached by keyboard through the editor */}
      <div data-slot="page-body" className="page-body" onClick={(event) => {
        const link = event.target instanceof Element ? event.target.closest("a[href]") : null;
        const href = link?.getAttribute("href");
        if (!href || !(event.metaKey || event.ctrlKey)) return;
        event.preventDefault();
        if (safeLink(href)) onOpenLink(href);
      }}>
        <RichTextPlugin
          contentEditable={<ContentEditable aria-label={label} className="page-content" spellCheck />}
          placeholder={null}
          ErrorBoundary={LexicalErrorBoundary}
        />
      </div>
      <HistoryPlugin />
      <ListPlugin />
      <CheckListPlugin />
      <LinkPlugin validateUrl={safeLink} />
      <TablePlugin hasCellMerge={false} hasCellBackgroundColor={false} />
      <HorizontalRulePlugin />
      <TabIndentationPlugin />
      <MarkdownShortcutPlugin transformers={PAGE_TRANSFORMERS} />
      <OnChangePlugin ignoreSelectionChange onChange={(state) => onChange(state.read(() => $exportPage()))} />
      <EditorRefPlugin editorRef={editorRef} />
      {children}
    </LexicalComposer>
  );
}

/** Puts the caret at the start of the page's first block. */
export function focusPageStart(editor: LexicalEditor): void {
  editor.focus(() => editor.update(() => { $getRoot().selectStart(); }));
}
