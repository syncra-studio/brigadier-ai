import { INTERNAL } from "@assistant-ui/react";
import type { Unstable_DirectiveFormatter } from "@assistant-ui/core";
import {
  DirectiveChipProvider,
  type DirectiveChipProps,
  DirectivePlugin,
  type DirectivePluginProps,
} from "@assistant-ui/react-lexical";
import { useAui, useAuiState } from "@assistant-ui/store";
import {
  $isCodeNode,
  CodeExtension,
  CodeHighlightNode,
  CodeNode,
} from "@lexical/code";
import { HistoryExtension } from "@lexical/history";
import { LinkExtension } from "@lexical/link";
import { $isListItemNode, $isListNode, CheckListExtension, ListExtension } from "@lexical/list";
import { ContentEditable } from "@lexical/react/LexicalContentEditable";
import { useLexicalComposerContext } from "@lexical/react/LexicalComposerContext";
import { LexicalExtensionComposer } from "@lexical/react/LexicalExtensionComposer";
import { MarkdownShortcutPlugin } from "@lexical/react/LexicalMarkdownShortcutPlugin";
import { RichTextExtension } from "@lexical/rich-text";
import { $findMatchingParent, mergeRegister } from "@lexical/utils";
import {
  $createLineBreakNode,
  $createParagraphNode,
  $createTabNode,
  $getNodeByKey,
  $getRoot,
  $getSelection,
  $isLineBreakNode,
  $isRangeSelection,
  $isTextNode,
  COMMAND_PRIORITY_CRITICAL,
  COMMAND_PRIORITY_HIGH,
  COMMAND_PRIORITY_LOW,
  configExtension,
  defineExtension,
  type EditorThemeClasses,
  FORMAT_TEXT_COMMAND,
  HISTORY_MERGE_TAG,
  INDENT_CONTENT_COMMAND,
  INSERT_PARAGRAPH_COMMAND,
  KEY_ARROW_DOWN_COMMAND,
  KEY_ARROW_UP_COMMAND,
  KEY_BACKSPACE_COMMAND,
  KEY_ENTER_COMMAND,
  KEY_ESCAPE_COMMAND,
  KEY_TAB_COMMAND,
  type LexicalNode,
  OUTDENT_CONTENT_COMMAND,
  SKIP_DOM_SELECTION_TAG,
  TextNode,
} from "lexical";
import {
  type ComponentPropsWithoutRef,
  type FC,
  forwardRef,
  type ReactNode,
  useEffect,
  useRef,
} from "react";

import {
  $exportComposer,
  $importComposer,
  CARET,
  COMPOSER_NODES,
  COMPOSER_TRANSFORMERS,
} from "@/app/conversation/composerMarkdown";

import { $applyColours } from "./codeColours";

import "./rich-composer-input.css";

/**
 * The rich composer field: assistant-ui's Lexical composer input made rich. Markdown typed in
 * it turns into formatting as it is typed (bold, italic, strike, code, links, lists, quotes,
 * headings, code blocks, rules), and the composer's text is its Markdown, so drafts, recalled
 * prompts and pulled queue messages come back formatted. Its chips (mentions, links, pasted
 * images) are assistant-ui's directive nodes, as in the plain field. No key formats text:
 * formatting is typed as Markdown.
 */

const THEME: EditorThemeClasses = {
  paragraph: "m-0",
  heading: {
    h1: "m-0 text-xl font-semibold",
    h2: "m-0 text-lg font-semibold",
    h3: "m-0 text-base font-semibold",
    h4: "m-0 text-base font-semibold",
    h5: "m-0 text-base font-semibold",
    h6: "m-0 text-base font-semibold text-muted-foreground",
  },
  quote: "m-0 border-s-2 border-border ps-3 text-muted-foreground",
  list: {
    ul: "m-0 list-disc ps-6",
    ol: "m-0 list-decimal ps-6",
    checklist: "m-0 list-none ps-1",
    listitem: "my-0",
    listitemChecked: "composer-check composer-check-on line-through text-muted-foreground",
    listitemUnchecked: "composer-check",
    nested: { listitem: "list-none" },
  },
  link: "text-link",
  code: "my-1 block whitespace-pre overflow-x-auto rounded-md bg-muted px-3 py-2 font-mono text-sm",
  hr: "composer-hr",
  text: {
    bold: "font-semibold",
    italic: "italic",
    strikethrough: "line-through",
    code: "bg-muted text-code-inline rounded-md px-1.5 py-0.5 font-mono",
  },
};

const richComposer = defineExtension({
  name: "brigadier/rich-composer",
  namespace: "brigadier-rich-composer",
  nodes: COMPOSER_NODES,
  theme: THEME,
  dependencies: [
    RichTextExtension,
    configExtension(HistoryExtension, { delay: 1000 }),
    ListExtension,
    CheckListExtension,
    LinkExtension,
    CodeExtension,
  ],
  onError: (error) => {
    console.error("[RichComposerInput]", error);
  },
});

/** Marks the field's updates made from the composer's text, which don't set it again. */
const SYNC_TAG = "brigadier-rich-sync";

/**
 * Keeps the composer's text and the field in step: an edit in the field sets the text to its
 * Markdown; text set from outside (a draft, ↑, a pulled queue message, a guess taken with Tab)
 * is read as Markdown, its chips as chips.
 */
function RichSyncPlugin({ formatter }: { formatter: Unstable_DirectiveFormatter }) {
  const [editor] = useLexicalComposerContext();
  const aui = useAui();
  const fromField = useRef(false);
  const last = useRef<string | null>(null);
  const formatterRef = useRef(formatter);
  useEffect(() => {
    formatterRef.current = formatter;
  }, [formatter]);

  useEffect(() => {
    const composer = aui.composer;
    return editor.registerUpdateListener(({ editorState, tags, dirtyElements, dirtyLeaves }) => {
      if (tags.has(SYNC_TAG)) return;
      if (dirtyElements.size === 0 && dirtyLeaves.size === 0) return;
      const markdown = editorState.read(() => $exportComposer());
      if (markdown === last.current) return;
      last.current = markdown;
      fromField.current = true;
      try {
        composer.setText(markdown);
      } finally {
        fromField.current = false;
      }
    });
    // The two hook rules disagree on `aui`; it is used, and stays the same.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [editor, aui]);

  useEffect(() => {
    // assistant-ui's own Lexical input listens to the runtime the same way.
    // oxlint-disable-next-line no-underscore-dangle
    const runtime = aui.composer.__internal_getRuntime?.();
    if (!runtime) return undefined;
    const apply = (text: string) => {
      last.current = text;
      const focused = editor.getRootElement()?.contains(document.activeElement) ?? false;
      editor.update(
        () => {
          const { parse } = formatterRef.current;
          $importComposer(text, parse, formatterRef.current);
          $getRoot().selectEnd();
        },
        { tag: focused ? SYNC_TAG : [SYNC_TAG, SKIP_DOM_SELECTION_TAG] },
      );
    };
    const initial = runtime.getState().text;
    if (initial !== last.current) apply(initial);
    return runtime.subscribe(() => {
      if (fromField.current) return;
      const text = runtime.getState().text;
      if (text !== last.current) apply(text);
    });
  }, [editor, aui]);
  return null;
}

/**
 * Tells the `@` and `/` menus where the caret is in the composer's text, which is Markdown:
 * an offset in it, measured by exporting with a marker at the caret. Only when the word at the
 * caret could open a menu; elsewhere the start, where none opens.
 */
function RichCursorPlugin() {
  const [editor] = useLexicalComposerContext();
  const registry = INTERNAL.useComposerInputPluginRegistryOptional();
  useEffect(() => {
    if (!registry) return undefined;
    let lastOffset = -1;
    return editor.registerUpdateListener(({ editorState }) => {
      const offset = editorState.read(() => {
        const selection = $getSelection();
        if (!$isRangeSelection(selection) || !selection.isCollapsed()) return 0;
        const { anchor } = selection;
        const node = anchor.getNode();
        if (anchor.type !== "text" || !$isTextNode(node) || $isCodeNode(node.getParent()) || node.hasFormat("code")) {
          return 0;
        }
        const word = node.getTextContent().slice(0, anchor.offset).split(/\s/).pop() ?? "";
        if (!/[@/]/.test(word)) return 0;
        const marked = $exportComposer({ key: node.getKey(), offset: anchor.offset });
        return Math.max(0, marked.indexOf(CARET));
      });
      if (offset === lastOffset) return;
      lastOffset = offset;
      for (const plugin of registry.getPlugins()) plugin.setCursorPosition(offset);
    });
  }, [editor, registry]);
  return null;
}

/** The menus (`@`, `/`) take their keys first, as in assistant-ui's own input. */
function MenuKeysPlugin() {
  const [editor] = useLexicalComposerContext();
  const registry = INTERNAL.useComposerInputPluginRegistryOptional();
  useEffect(() => {
    const delegate = (event: KeyboardEvent | null): boolean => {
      if (!event || !registry) return false;
      for (const plugin of registry.getPlugins()) if (plugin.handleKeyDown(event)) return true;
      return false;
    };
    return mergeRegister(
      editor.registerCommand(
        KEY_ENTER_COMMAND,
        (event) => !!event && !event.isComposing && !event.shiftKey && delegate(event),
        COMMAND_PRIORITY_HIGH,
      ),
      ...[KEY_ESCAPE_COMMAND, KEY_ARROW_DOWN_COMMAND, KEY_ARROW_UP_COMMAND, KEY_BACKSPACE_COMMAND, KEY_TAB_COMMAND].map(
        (command) => editor.registerCommand(command, delegate, COMMAND_PRIORITY_HIGH),
      ),
    );
  }, [editor, registry]);
  return null;
}

/** The code block holding the caret, if it is in one. */
function $caretCode(): CodeNode | null {
  const selection = $getSelection();
  if (!$isRangeSelection(selection)) return null;
  const node = selection.anchor.getNode();
  return $isCodeNode(node) ? node : ($findMatchingParent(node, $isCodeNode) as CodeNode | null);
}

/** Whether the caret is collapsed at the very end of `code`. */
function $atCodeEnd(code: CodeNode): boolean {
  const selection = $getSelection();
  if (!$isRangeSelection(selection) || !selection.isCollapsed()) return false;
  const last = code.getLastDescendant();
  const { anchor } = selection;
  if (!last) return true;
  if (anchor.key === code.getKey()) return anchor.offset === code.getChildrenSize();
  return anchor.key === last.getKey() && anchor.offset === last.getTextContentSize();
}

/** Leaves `code` for a line after it (made if there is none), dropping `drop` from its end. */
function $leaveCode(code: CodeNode, drop: LexicalNode[]) {
  for (const node of drop) node.remove();
  const next = code.getNextSibling();
  if (next) {
    next.selectStart();
    return;
  }
  const line = $createParagraphNode();
  code.insertAfter(line);
  line.select();
}

/** Leaves `code` if its last line is a closing ``` (dropped with the line break before it). */
function $closeFence(code: CodeNode) {
  const children = code.getChildren();
  const fence = children.at(-1);
  const before = children.at(-2);
  if (!fence || !before || fence.getTextContent() !== "```" || !$isLineBreakNode(before)) return;
  const selection = $getSelection();
  if (!$isRangeSelection(selection) || selection.anchor.getNode() !== fence) return;
  $leaveCode(code, [before, fence]);
}

/**
 * `- [ ] ` typed as one line makes a task item: `- ` has already made a bullet, and Markdown's
 * own task rule only reads a line outside lists. The bullet list of just this item turns into a
 * task list once the space after `[ ]` or `[x]` is typed.
 */
function $taskFromBullet(node: TextNode) {
  const item = node.getParent();
  const list = item?.getParent();
  if (!$isListItemNode(item) || !$isListNode(list) || list.getListType() !== "bullet") return;
  if (list.getChildrenSize() !== 1 || item.getFirstChild() !== node) return;
  const marker = /^\[( |x)?\] /i.exec(node.getTextContent());
  const selection = $getSelection();
  if (!marker || !$isRangeSelection(selection) || !selection.isCollapsed()) return;
  if (selection.anchor.getNode() !== node || selection.anchor.offset !== marker[0].length) return;
  list.setListType("check");
  item.setChecked(marker[1]?.toLowerCase() === "x");
  if (node.getTextContentSize() === marker[0].length) {
    item.select(0, 0);
    node.remove();
  } else node.spliceText(0, marker[0].length, "", true);
}

/**
 * The rich field's own keys. Shift+Enter starts a new line: a new paragraph, the next list item
 * (out of the list on an empty one), a new line in a code block, which an empty last line
 * leaves. A closing ``` line or ↓ on its last line leave a code block too. `- [ ] ` makes a task
 * item. Tab and ⇧Tab indent a list item, and put a tab in code. Keys that would format text do
 * nothing: formatting is typed as Markdown, and ⌘B stays the sidebar's.
 */
function RichKeysPlugin() {
  const [editor] = useLexicalComposerContext();
  useEffect(
    () =>
      mergeRegister(
        editor.registerCommand(FORMAT_TEXT_COMMAND, () => true, COMMAND_PRIORITY_CRITICAL),
        editor.registerCommand(
          KEY_ENTER_COMMAND,
          (event) => {
            if (!event?.shiftKey || event.isComposing || event.altKey || event.metaKey || event.ctrlKey) return false;
            event.preventDefault();
            const code = $caretCode();
            if (!code) return editor.dispatchCommand(INSERT_PARAGRAPH_COMMAND, undefined);
            const last = code.getLastChild();
            if ($atCodeEnd(code) && (code.getChildrenSize() === 0 || $isLineBreakNode(last))) {
              $leaveCode(code, last ? [last] : []);
              return true;
            }
            const selection = $getSelection();
            if ($isRangeSelection(selection)) selection.insertNodes([$createLineBreakNode()]);
            return true;
          },
          COMMAND_PRIORITY_LOW,
        ),
        editor.registerCommand(
          KEY_ARROW_DOWN_COMMAND,
          (event) => {
            const code = $caretCode();
            if (!code || event.shiftKey) return false;
            const selection = $getSelection();
            if (!$isRangeSelection(selection) || !selection.isCollapsed()) return false;
            // On the last line: no line break after the caret.
            let node: LexicalNode | null = selection.anchor.getNode();
            if (node === code) node = code.getChildAtIndex(selection.anchor.offset);
            for (; node; node = node.getNextSibling()) if ($isLineBreakNode(node)) return false;
            event.preventDefault();
            $leaveCode(code, []);
            return true;
          },
          COMMAND_PRIORITY_LOW,
        ),
        editor.registerCommand(
          KEY_TAB_COMMAND,
          (event) => {
            const selection = $getSelection();
            if (!$isRangeSelection(selection)) return false;
            if ($caretCode()) {
              event.preventDefault();
              if (!event.shiftKey) selection.insertNodes([$createTabNode()]);
              return true;
            }
            if (!$findMatchingParent(selection.anchor.getNode(), $isListItemNode)) return false;
            event.preventDefault();
            return editor.dispatchCommand(event.shiftKey ? OUTDENT_CONTENT_COMMAND : INDENT_CONTENT_COMMAND, undefined);
          },
          COMMAND_PRIORITY_LOW,
        ),
        // A line of just ``` at the end of a code block closes it.
        ...[TextNode, CodeHighlightNode].map((klass) =>
          editor.registerNodeTransform(klass, (node) => {
            const code = node.getParent();
            if ($isCodeNode(code)) $closeFence(code);
          }),
        ),
        editor.registerNodeTransform(TextNode, $taskFromBullet),
      ),
    [editor],
  );
  return null;
}

/** shiki's colours for code blocks with a language, once its grammar has loaded. */
function CodeColoursPlugin() {
  const [editor] = useLexicalComposerContext();
  useEffect(() => {
    let live = true;
    const pending = new Map<string, number>();
    const colour = (key: string) => {
      const version = (pending.get(key) ?? 0) + 1;
      pending.set(key, version);
      const read = editor.getEditorState().read(() => {
        const code = $getNodeByKey(key);
        return $isCodeNode(code) ? { text: code.getTextContent(), language: code.getLanguage() ?? "" } : null;
      });
      if (!read?.language) return;
      void import("@/lib/highlight")
        .then(({ highlight }) => highlight(read.text, read.language))
        .then((lines) => {
          if (!live || !lines || pending.get(key) !== version) return;
          editor.update(() => $applyColours(key, read.text, lines), { tag: HISTORY_MERGE_TAG });
        })
        .catch(() => undefined);
    };
    return mergeRegister(
      () => {
        live = false;
      },
      editor.registerMutationListener(CodeNode, (mutations) => {
        for (const [key, mutation] of mutations) if (mutation !== "destroyed") colour(key);
      }),
      editor.registerUpdateListener(({ dirtyLeaves, editorState }) => {
        if (dirtyLeaves.size === 0) return;
        const codes = editorState.read(() => {
          const keys = new Set<string>();
          for (const leaf of dirtyLeaves) {
            const node = $getNodeByKey(leaf);
            const code = node && ($findMatchingParent(node, $isCodeNode) as CodeNode | null);
            if (code) keys.add(code.getKey());
          }
          return keys;
        });
        for (const key of codes) colour(key);
      }),
    );
  }, [editor]);
  return null;
}

/** Focuses the field on mount if asked, and when a run starts (as assistant-ui's input does). */
function FocusPlugin({ autoFocus }: { autoFocus: boolean }) {
  const [editor] = useLexicalComposerContext();
  const aui = useAui();
  useEffect(() => {
    if (autoFocus) editor.focus();
  }, [editor, autoFocus]);
  useEffect(() => aui.on("thread.runStart", () => editor.focus()), [editor, aui]);
  return null;
}

/** Read-only while the thread is disabled or dictation holds the field. */
function EditablePlugin() {
  const [editor] = useLexicalComposerContext();
  const disabled = useAuiState((s) => s.thread.isDisabled || s.composer.dictation?.inputDisabled);
  useEffect(() => {
    editor.setEditable(!disabled);
  }, [editor, disabled]);
  return null;
}

export type RichComposerInputProps = Omit<ComponentPropsWithoutRef<"div">, "autoFocus" | "children"> & {
  placeholder?: string | undefined;
  autoFocus?: boolean | undefined;
  directivePluginProps?: DirectivePluginProps | undefined;
  directiveChip?: FC<DirectiveChipProps> | undefined;
  formatter: Unstable_DirectiveFormatter;
  /** Further Lexical plugins. */
  children?: ReactNode | undefined;
};

export const RichComposerInput = forwardRef<HTMLDivElement, RichComposerInputProps>(
  (
    {
      placeholder,
      autoFocus = false,
      directivePluginProps,
      directiveChip,
      formatter,
      "aria-label": ariaLabel,
      "aria-labelledby": ariaLabelledBy,
      "aria-describedby": ariaDescribedBy,
      className,
      children,
      ...rest
    },
    ref,
  ) => (
    <DirectiveChipProvider value={directiveChip ?? null}>
      <LexicalExtensionComposer extension={richComposer} contentEditable={null}>
        <div
          ref={ref}
          className={className ? `aui-lexical-editor overflow-y-auto ${className}` : "aui-lexical-editor overflow-y-auto"}
          data-rich-composer=""
          {...rest}
        >
          <ContentEditable
            className="aui-lexical-input"
            aria-label={ariaLabel ?? (ariaLabelledBy ? undefined : placeholder)}
            aria-labelledby={ariaLabelledBy}
            aria-describedby={ariaDescribedBy}
            {...(placeholder
              ? {
                  placeholder: <div className="aui-lexical-placeholder">{placeholder}</div>,
                  "aria-placeholder": placeholder,
                }
              : {})}
          />
          <RichSyncPlugin formatter={formatter} />
          <DirectivePlugin {...directivePluginProps} />
          <MarkdownShortcutPlugin transformers={COMPOSER_TRANSFORMERS} />
          <MenuKeysPlugin />
          <RichKeysPlugin />
          <RichCursorPlugin />
          <CodeColoursPlugin />
          <FocusPlugin autoFocus={autoFocus} />
          <EditablePlugin />
          {children}
        </div>
      </LexicalExtensionComposer>
    </DirectiveChipProvider>
  ),
);

RichComposerInput.displayName = "RichComposerInput";

