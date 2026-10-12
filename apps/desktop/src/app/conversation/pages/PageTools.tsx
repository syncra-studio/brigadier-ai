import { $isCodeNode } from "@lexical/code";
import { $isLinkNode, TOGGLE_LINK_COMMAND } from "@lexical/link";
import { useLexicalComposerContext } from "@lexical/react/LexicalComposerContext";
import { DraggableBlockPlugin_EXPERIMENTAL } from "@lexical/react/LexicalDraggableBlockPlugin";
import { LexicalTypeaheadMenuPlugin, MenuOption, type MenuRenderFn, useBasicTypeaheadTriggerMatch } from "@lexical/react/LexicalTypeaheadMenuPlugin";
import { $findMatchingParent, mergeRegister } from "@lexical/utils";
import { ChevronDown, Code, Link } from "@openai/apps-sdk-ui/components/Icon";
import {
  $getSelection,
  $isRangeSelection,
  BLUR_COMMAND,
  COMMAND_PRIORITY_LOW,
  FORMAT_TEXT_COMMAND,
  KEY_DOWN_COMMAND,
  SELECTION_CHANGE_COMMAND,
  type TextFormatType,
} from "lexical";
import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";

import { $currentBlock, applyBlock, type Block, BLOCKS, blockMatches, type BlockKind, TURN_INTO } from "./pageBlocks";

/** A row of the Insert list: its icon, its name and its Markdown, right-aligned. */
export function BlockRow({ block }: { block: Block }) {
  return (
    <>
      {block.icon}
      <span className="flex-1">{block.label}</span>
      {block.markdown && <span className="page-menu-hint">{block.markdown}</span>}
    </>
  );
}

class BlockOption extends MenuOption {
  constructor(readonly block: Block) { super(block.kind); }
}

const renderSlashMenu: MenuRenderFn<BlockOption> = (anchor, { options, selectedIndex, selectOptionAndCleanUp, setHighlightedIndex }) =>
  anchor.current && options.length ? createPortal(
    <div role="listbox" aria-label="Insert options" data-slot="page-insert-menu" className="page-menu page-slash-menu">
      <div className="page-menu-label">Basic</div>
      {options.map((option, index) => (
        <div
          key={option.key}
          ref={(element) => option.setRefElement(element)}
          role="option"
          aria-selected={index === selectedIndex}
          tabIndex={-1}
          className="page-menu-item"
          onMouseEnter={() => setHighlightedIndex(index)}
          onMouseDown={(event) => event.preventDefault()}
          onClick={() => selectOptionAndCleanUp(option)}
          onKeyDown={(event) => { if (event.key === "Enter") selectOptionAndCleanUp(option); }}
        >
          <BlockRow block={option.block} />
        </div>
      ))}
    </div>,
    anchor.current,
  ) : null;

/** `/` at a line's start (or after a space) opens the Insert list; typing narrows it. */
export function SlashMenu() {
  const [editor] = useLexicalComposerContext();
  const [query, setQuery] = useState<string | null>(null);
  const trigger = useBasicTypeaheadTriggerMatch("/", { minLength: 0 });
  const options = useMemo(() => BLOCKS.filter((block) => block.kind !== "paragraph" && blockMatches(block, query ?? ""))
    .map((block) => new BlockOption(block)), [query]);
  const select = useCallback((option: BlockOption, typed: { remove: () => void } | null, close: () => void) => {
    editor.update(() => { typed?.remove(); });
    applyBlock(editor, option.block.kind);
    close();
  }, [editor]);
  return (
    <LexicalTypeaheadMenuPlugin<BlockOption>
      onQueryChange={setQuery}
      onSelectOption={select}
      triggerFn={trigger}
      options={options}
      menuRenderFn={renderSlashMenu}
    />
  );
}

type Toolbar = { top: number; left: number; formats: Set<TextFormatType>; link: boolean; block: BlockKind | null };

const FORMATS: { format: TextFormatType; label: string; shortcut: string; glyph: ReactNode }[] = [
  { format: "bold", label: "Bold", shortcut: "⌘B", glyph: <span className="page-glyph font-semibold">B</span> },
  { format: "italic", label: "Italic", shortcut: "⌘I", glyph: <span className="page-glyph font-serif italic">I</span> },
  { format: "underline", label: "Underline", shortcut: "⌘U", glyph: <span className="page-glyph underline">U</span> },
  { format: "strikethrough", label: "Strikethrough", shortcut: "⌘⇧X", glyph: <span className="page-glyph line-through">S</span> },
  { format: "code", label: "Code", shortcut: "⌘E", glyph: <Code /> },
];

/** A link typed without a scheme opens as a web address. */
export function linkTarget(typed: string): string | null {
  const text = typed.trim();
  if (!text) return null;
  if (/^(?:https?:|mailto:)/i.test(text)) return text;
  if (/^[a-z][a-z0-9+.-]*:/i.test(text)) return null;
  return `https://${text}`;
}

/** Above a selection of text: B I U S, code, link, and the block's kind. */
export function SelectionToolbar({ mac }: { mac: boolean }) {
  const [editor] = useLexicalComposerContext();
  const [bar, setBar] = useState<Toolbar | null>(null);
  const [linking, setLinking] = useState<string | null>(null);
  const element = useRef<HTMLDivElement | null>(null);
  const refresh = useCallback(() => {
    const native = window.getSelection();
    const root = editor.getRootElement();
    const found = editor.getEditorState().read(() => {
      const selection = $getSelection();
      if (!$isRangeSelection(selection) || selection.isCollapsed() || !selection.getTextContent().trim()) return null;
      const anchor = selection.anchor.getNode();
      if ($findMatchingParent(anchor, $isCodeNode)) return null;
      const formats = new Set(FORMATS.map((entry) => entry.format).filter((format) => selection.hasFormat(format)));
      const link = selection.getNodes().some((node) => $isLinkNode(node) || $isLinkNode(node.getParent()));
      return { formats, link, block: $currentBlock() };
    });
    if (!found || !native || native.rangeCount === 0 || !root || !root.contains(native.anchorNode)) {
      // Focus moving into the toolbar itself keeps it.
      if (!element.current?.contains(document.activeElement)) { setBar(null); setLinking(null); }
      return;
    }
    const rect = native.getRangeAt(0).getBoundingClientRect();
    // Kept whole on screen: its centre stays half its width from either edge.
    const half = (element.current?.offsetWidth ?? 0) / 2 + 8;
    setBar({ top: rect.top, left: Math.min(Math.max(rect.left + rect.width / 2, half), window.innerWidth - half), ...found });
  }, [editor]);
  useEffect(() => mergeRegister(
    editor.registerUpdateListener(() => refresh()),
    editor.registerCommand(SELECTION_CHANGE_COMMAND, () => { refresh(); return false; }, COMMAND_PRIORITY_LOW),
    editor.registerCommand(BLUR_COMMAND, () => { setTimeout(refresh, 0); return false; }, COMMAND_PRIORITY_LOW),
    editor.registerCommand(KEY_DOWN_COMMAND, (event) => {
      const command = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
      if (!command || event.altKey) return false;
      const format: TextFormatType | null = event.shiftKey && event.code === "KeyX" ? "strikethrough" : !event.shiftKey && event.code === "KeyE" ? "code" : null;
      if (!format) return false;
      event.preventDefault();
      editor.dispatchCommand(FORMAT_TEXT_COMMAND, format);
      return true;
    }, COMMAND_PRIORITY_LOW),
  ), [editor, mac, refresh]);
  useEffect(() => {
    if (!bar) return;
    const scrolled = () => refresh();
    window.addEventListener("scroll", scrolled, true);
    window.addEventListener("resize", scrolled);
    return () => { window.removeEventListener("scroll", scrolled, true); window.removeEventListener("resize", scrolled); };
  }, [bar, refresh]);
  if (!bar) return null;
  const finishLink = () => {
    const url = linkTarget(linking ?? "");
    setLinking(null);
    if (url) editor.dispatchCommand(TOGGLE_LINK_COMMAND, url);
    editor.focus();
  };
  const current = TURN_INTO.find((block) => block.kind === bar.block) ?? TURN_INTO[0]!;
  return createPortal(
    <div
      ref={element}
      role="toolbar"
      tabIndex={-1}
      aria-label="Format text"
      data-slot="page-selection-toolbar"
      className="page-selection-toolbar"
      style={{ top: `${bar.top}px`, left: `${bar.left}px` }}
      onMouseDown={(event) => { if (!(event.target instanceof HTMLInputElement)) event.preventDefault(); }}
    >
      {linking !== null ? (
        <input
          // The link field takes the keyboard as soon as Link is pressed.
          // oxlint-disable-next-line jsx-a11y/no-autofocus
          autoFocus
          aria-label="Link address"
          placeholder="Paste or type a link"
          className="page-link-field"
          value={linking}
          onChange={(event) => setLinking(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") { event.preventDefault(); finishLink(); }
            if (event.key === "Escape") { event.preventDefault(); setLinking(null); editor.focus(); }
          }}
          onBlur={() => setLinking(null)}
        />
      ) : (
        <>
          {FORMATS.map(({ format, label, shortcut, glyph }) => (
            <TooltipIconButton key={format} tooltip={label} shortcut={shortcut} side="top" size="icon-md" aria-pressed={bar.formats.has(format)}
              className={cn("page-toolbar-button", bar.formats.has(format) && "bg-foreground/8")}
              onClick={() => editor.dispatchCommand(FORMAT_TEXT_COMMAND, format)}>
              {glyph}
            </TooltipIconButton>
          ))}
          <TooltipIconButton tooltip={bar.link ? "Remove link" : "Link"} side="top" size="icon-md" aria-pressed={bar.link}
            className={cn("page-toolbar-button", bar.link && "bg-foreground/8")}
            onClick={() => (bar.link ? editor.dispatchCommand(TOGGLE_LINK_COMMAND, null) : setLinking(""))}>
            <Link />
          </TooltipIconButton>
          <span aria-hidden className="page-toolbar-divider" />
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button type="button" aria-label={`Turn into, now ${current.label}`} className="page-toolbar-text">
                <span>{current.label === "Text" ? "Text" : current.label}</span><ChevronDown aria-hidden className="size-icon-sm" />
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" className="page-menu" onCloseAutoFocus={(event) => { event.preventDefault(); editor.focus(); }}>
              {TURN_INTO.map((block) => (
                <DropdownMenuItem key={block.kind} className="page-menu-item" data-current={block.kind === bar.block || undefined}
                  onSelect={() => applyBlock(editor, block.kind)}>
                  <BlockRow block={block} />
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        </>
      )}
    </div>,
    document.body,
  );
}

/** Six dots beside the block under the pointer: drag them to move the block. */
export function DragHandles({ anchor }: { anchor: HTMLElement | null }) {
  const menu = useRef<HTMLDivElement | null>(null);
  const line = useRef<HTMLDivElement | null>(null);
  if (!anchor) return null;
  return (
    <DraggableBlockPlugin_EXPERIMENTAL
      anchorElem={anchor}
      menuRef={menu}
      targetLineRef={line}
      menuComponent={
        <div ref={menu} data-slot="page-drag-handle" aria-hidden className="page-drag-handle">
          <svg viewBox="0 0 10 16" fill="currentColor" className="page-drag-dots">
            {[3, 8, 13].flatMap((y) => [2.5, 7.5].map((x) => <circle key={`${x}-${y}`} cx={x} cy={y} r="1.25" />))}
          </svg>
        </div>
      }
      targetLineComponent={<div ref={line} className="page-drop-line" />}
      isOnMenu={(element) => element.closest(".page-drag-handle") !== null}
    />
  );
}
