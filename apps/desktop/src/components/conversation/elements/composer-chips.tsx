import type { Unstable_DirectiveFormatter, Unstable_TriggerItem } from "@assistant-ui/react";
import {
  $createDirectiveNodeWithFormatter,
  $isDirectiveNode,
  type DirectiveChipProps,
  type DirectiveNode,
  LexicalComposerInput,
  type LexicalComposerInputProps,
} from "@assistant-ui/react-lexical";
import { $isCodeNode } from "@lexical/code";
import { useLexicalComposerContext } from "@lexical/react/LexicalComposerContext";
import { mergeRegister } from "@lexical/utils";
import {
  $createTextNode,
  $getNearestNodeFromDOMNode,
  $getSelection,
  $hasUpdateTag,
  $isElementNode,
  $isRangeSelection,
  $isTextNode,
  COMMAND_PRIORITY_NORMAL,
  DELETE_CHARACTER_COMMAND,
  HISTORY_MERGE_TAG,
  KEY_ARROW_LEFT_COMMAND,
  KEY_ARROW_RIGHT_COMMAND,
  KEY_BACKSPACE_COMMAND,
  KEY_DOWN_COMMAND,
  type LexicalEditor,
  PASTE_TAG,
  type PointType,
  TextNode,
} from "lexical";
import {
  createContext,
  type FC,
  type MouseEvent,
  type ReactNode,
  type RefObject,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { RichComposerInput } from "@/components/conversation/elements/rich-composer-input";
import {
  ALL_CHIPS,
  type ChipKinds,
  CODE,
  completedTokens,
  IMAGE_CHIP,
  PASTE_CHIP,
  SPACE,
  URL_CHIP,
} from "@/components/conversation/elements/chipText";
import { FileTypeIcon } from "@/components/conversation/elements/file-type-icon";
import { ComposerInlineImage } from "@/components/conversation/elements/inline-image";
import { ComposerPastedText } from "@/components/conversation/elements/pasted-text";
import { Globe } from "@/components/icons";
import { cn } from "@/lib/utils";

export {
  ALL_CHIPS,
  type ChipKinds,
  type ChipMention,
  chipFormatter,
  imageChip,
  type ImageMatcher,
  type MentionMatcher,
  parseChips,
  pasteChip,
} from "@/components/conversation/elements/chipText";

/** The app's icon and name for a mention chip of `type` (a worker's glyph and name, a chat's title). */
export type MentionLook = (chip: DirectiveChipProps) => { icon: ReactNode; name: ReactNode } | null;

const MentionLookContext = createContext<MentionLook | null>(null);

const CHIP_ICON = "size-icon-sm mr-0.5 inline-block align-text-bottom";

function fileIcon(name: string): ReactNode {
  return <FileTypeIcon name={name} className={CHIP_ICON} />;
}

/**
 * One chip, inline in the composer's text: a blue mention, mono code pill, link, a pasted
 * image's thumbnail or a pasted text.
 */
const ComposerChip: FC<DirectiveChipProps> = (chip) => {
  const look = useContext(MentionLookContext);
  if (chip.directiveType === IMAGE_CHIP) {
    return <ComposerImageChip n={Number(chip.directiveId.slice(IMAGE_CHIP.length + 1))} />;
  }
  if (chip.directiveType === PASTE_CHIP) {
    return <ComposerImageChip n={Number(chip.directiveId.slice(PASTE_CHIP.length + 1))} paste />;
  }
  if (chip.directiveType === CODE) {
    return (
      <span data-slot="composer-chip" data-chip={CODE} className="bg-muted text-code-inline rounded-md px-1.5 py-0.5 font-mono">
        {chip.label}
      </span>
    );
  }
  const url = chip.directiveType === URL_CHIP;
  const custom = url ? null : look?.(chip);
  const base = chip.label.slice(chip.label.lastIndexOf("/") + 1);
  return (
    <span data-slot="composer-chip" data-chip={chip.directiveType} className="text-link">
      {custom ? (
        <span className="[&_svg]:size-icon-sm mr-0.5 inline-flex align-text-bottom">{custom.icon}</span>
      ) : url ? (
        <Globe aria-hidden className={CHIP_ICON} />
      ) : (
        fileIcon(base)
      )}
      {custom?.name ?? (url ? chip.label : base)}
    </span>
  );
};

/**
 * A pasted image's chip (or, with `paste`, a pasted text's), whose × takes it out of the text
 * as Backspace would: its image goes with it, and undo brings both back.
 */
function ComposerImageChip({ n, paste = false }: { n: number; paste?: boolean }) {
  const [editor] = useLexicalComposerContext();
  const remove = (event: MouseEvent<HTMLButtonElement>) => {
    const chip = event.currentTarget;
    editor.update(() => $getNearestNodeFromDOMNode(chip)?.remove());
    editor.focus();
  };
  return paste ? <ComposerPastedText n={n} onRemove={remove} /> : <ComposerInlineImage n={n} onRemove={remove} />;
}

/** Code and link chips, which Backspace turns back into text. */
const isTextChip = (node: DirectiveNode) => {
  const type = node.getDirectiveItem().type;
  return type === CODE || type === URL_CHIP;
};

/** The chip right before (or after) `point`, if any. */
function $chipBeside(point: PointType, backward: boolean): DirectiveNode | null {
  const node = point.getNode();
  const beside = $isTextNode(node)
    ? backward
      ? point.offset === 0
        ? node.getPreviousSibling()
        : null
      : point.offset === node.getTextContentSize()
        ? node.getNextSibling()
        : null
    : $isElementNode(node)
      ? node.getChildAtIndex(backward ? point.offset - 1 : point.offset)
      : null;
  return $isDirectiveNode(beside) ? beside : null;
}

/** The chip just before a collapsed caret, if any. */
function $chipBeforeCaret(): DirectiveNode | null {
  const selection = $getSelection();
  if (!$isRangeSelection(selection) || !selection.isCollapsed()) return null;
  return $chipBeside(selection.anchor, true);
}

/**
 * ← and → step over a chip as over one character (⇧ extends the selection over it). Lexical
 * stops the caret at a chip, since the input's chips are isolated nodes, which left no way past
 * one by keyboard: the caret stayed before it and what was typed next went in there.
 */
function $stepOverChip(event: KeyboardEvent, backward: boolean): boolean {
  if (event.isComposing || event.altKey || event.metaKey || event.ctrlKey) return false;
  const selection = $getSelection();
  if (!$isRangeSelection(selection) || (!event.shiftKey && !selection.isCollapsed())) return false;
  const chip = $chipBeside(selection.focus, backward);
  if (!chip) return false;
  const { key, offset, type } = selection.anchor;
  const moved = backward ? chip.selectPrevious() : chip.selectNext(0, 0);
  if (event.shiftKey) moved.anchor.set(key, offset, type);
  event.preventDefault();
  return true;
}

/**
 * Backspace right after a code or link chip turns it back into its text, less one character;
 * it stays text until its token is completed again (see `ChipsPlugin`).
 */
function $unwrapChipBeforeCaret(): boolean {
  const chip = $chipBeforeCaret();
  if (!chip || !isTextChip(chip)) return false;
  const text = chip.getDirectiveText().slice(0, -1);
  const raw = $createTextNode(text);
  chip.replace(raw);
  raw.select(text.length, text.length);
  return true;
}


/**
 * What turns off the webview's text checking (smart quotes and dashes, auto-correct, capitals)
 * and macOS's inline predictions, which ended up in the text (`seq` → `sequence`, #546).
 */
const PLAIN_TYPING = { spellcheck: "false", autocorrect: "off", autocapitalize: "off", writingsuggestions: "false" };

function plainTyping(root: HTMLElement) {
  for (const [name, value] of Object.entries(PLAIN_TYPING)) {
    // Only a change: setting the same value again would wake the observer for nothing.
    if (root.getAttribute(name) !== value) root.setAttribute(name, value);
  }
}

/**
 * macOS's "Add period with double-space" comes from the input method, past `spellcheck`: it
 * replaces the space before the caret with ". ". The field keeps the two spaces typed instead.
 */
function keepDoubleSpace(event: InputEvent) {
  if (event.data !== ". " || !/^insert(Replacement)?Text$/.test(event.inputType)) return;
  const [range] = event.getTargetRanges();
  if (!range || range.endOffset - range.startOffset !== 1 || range.startContainer !== range.endContainer) return;
  if (range.startContainer.textContent?.[range.startOffset] !== " ") return;
  event.preventDefault();
  event.stopImmediatePropagation();
  const root = event.currentTarget as HTMLElement;
  // The space stays; the second one goes in at the caret.
  root.ownerDocument.execCommand("insertText", false, " ");
}

/**
 * Turns `` `code` `` and links into chips: as they're typed, when the character completing one
 * (the closing backtick, the space after a link) goes in at the caret, and everywhere in a
 * paste. So a chip turned back into text stays text while it's edited. Never mid-composition.
 * It also keeps the system's text substitutions out of the field, lets ← and → past chips, and
 * leaves Enter and Space on a chip's own button (an image's preview or ×, reached by Tab) to
 * press it, where the editor would send the message.
 */
function ChipsPlugin({
  formatter,
  kinds,
  editorRef,
}: {
  formatter: Unstable_DirectiveFormatter;
  kinds: ChipKinds;
  /** Where the input keeps the editor, for work after a menu pick. */
  editorRef: RefObject<LexicalEditor | null>;
}) {
  const [editor] = useLexicalComposerContext();
  useEffect(() => {
    editorRef.current = editor;
  }, [editor, editorRef]);
  useEffect(() => {
    let kept: MutationObserver | null = null;
    return mergeRegister(
      // What's typed is what's sent: macOS turns "..." into "…", quotes into curly ones,
      // "--" into a dash and capitalizes words as part of its text checking, which only
      // `spellcheck="false"` turns off in the webview (`autocorrect="off"` does nothing there);
      // its inline predictions ("sequence" for `seq`) only `writingsuggestions="false"` does.
      // Lexical's focus() drops `autocapitalize`, so the field sets them again when one goes.
      editor.registerRootListener((root, previous) => {
        previous?.removeEventListener("beforeinput", keepDoubleSpace, true);
        kept?.disconnect();
        kept = null;
        if (!root) return;
        root.addEventListener("beforeinput", keepDoubleSpace, true);
        plainTyping(root);
        kept = new MutationObserver(() => plainTyping(root));
        kept.observe(root, { attributeFilter: Object.keys(PLAIN_TYPING) });
      }),
      () => {
        kept?.disconnect();
        editor.getRootElement()?.removeEventListener("beforeinput", keepDoubleSpace, true);
      },
      editor.registerNodeTransform(TextNode, (node) => {
        if (editor.isComposing() || !node.isSimpleText() || node.hasFormat("code")) return;
        if ($isCodeNode(node.getParent())) return;
        const selection = $getSelection();
        const caret =
          $isRangeSelection(selection) &&
          selection.isCollapsed() &&
          selection.anchor.key === node.getKey()
            ? selection.anchor.offset
            : -1;
        const pasted = $hasUpdateTag(PASTE_TAG);
        if (!pasted && caret < 0) return;
        // One chip per pass: the rest of the text is transformed again.
        const token = completedTokens(node.getTextContent(), pasted ? null : caret, kinds)[0];
        if (!token || token.segment.kind !== "mention") return;
        const { type, label, id } = token.segment;
        const parts = node.splitText(token.start, token.end);
        const target = token.start === 0 ? parts[0] : parts[1];
        if (!target) return;
        const chip = $createDirectiveNodeWithFormatter({ id, type, label }, formatter);
        // The caret right after the token's end goes on after the chip.
        const after = target.isSelected();
        target.replace(chip);
        if (after) chip.selectNext(0, 0);
      }),
      editor.registerCommand(
        KEY_DOWN_COMMAND,
        (event) =>
          (event.key === "Enter" || event.key === " ") &&
          event.target instanceof HTMLButtonElement,
        COMMAND_PRIORITY_NORMAL,
      ),
      editor.registerCommand(
        KEY_ARROW_LEFT_COMMAND,
        (event) => $stepOverChip(event, true),
        COMMAND_PRIORITY_NORMAL,
      ),
      editor.registerCommand(
        KEY_ARROW_RIGHT_COMMAND,
        (event) => $stepOverChip(event, false),
        COMMAND_PRIORITY_NORMAL,
      ),
      editor.registerCommand(
        KEY_BACKSPACE_COMMAND,
        (event) => {
          if (event.isComposing || !$unwrapChipBeforeCaret()) return false;
          event.preventDefault();
          return true;
        },
        COMMAND_PRIORITY_NORMAL,
      ),
      editor.registerCommand(
        DELETE_CHARACTER_COMMAND,
        (isBackward) => isBackward && $unwrapChipBeforeCaret(),
        COMMAND_PRIORITY_NORMAL,
      ),
    );
  }, [editor, formatter, kinds]);
  return null;
}

/**
 * assistant-ui's Lexical composer input with inline chips: a mention shows its icon
 * and blue name, `` `code` `` a mono pill, a link a globe and blue text, while the composer's
 * text stays plain. `rich`, it is the rich field instead (rich-composer-input), whose text is
 * Markdown and whose code is a mark, not a chip. It grows with its text up to
 * `max-h-composer-max`, then scrolls, the top line fading under the edge. `children` are
 * further Lexical plugins.
 */
export const ChipComposerInput: FC<
  Omit<LexicalComposerInputProps, "formatter" | "directiveChip" | "directivePluginProps"> & {
    formatter: Unstable_DirectiveFormatter;
    mentionLook?: MentionLook;
    /** Hears each item picked from a menu (it's in the text as a chip by then). */
    onMention?: (item: Unstable_TriggerItem) => void;
    /** One line (under an action card): it doesn't grow, it scrolls. */
    line?: boolean;
    /** The rich field, whose text is Markdown. */
    rich?: boolean;
    /** The chips its text makes (all of them by default). */
    kinds?: ChipKinds;
  }
> = ({ formatter, mentionLook, onMention, line = false, rich = false, kinds = ALL_CHIPS, className, children, ...props }) => {
  const [scrolled, setScrolled] = useState(false);
  const editor = useRef<LexicalEditor | null>(null);
  const directivePluginProps = useMemo(
    () => ({
      onDirectiveSelect: (item: Unstable_TriggerItem) => {
        // A space after the chip, as a picked mention gets.
        editor.current?.update(
          () => {
            const selection = $getSelection();
            if (!$isRangeSelection(selection) || !selection.isCollapsed()) return;
            const { anchor } = selection;
            const next = $isTextNode(anchor.getNode())
              ? anchor.getNode().getTextContent()[anchor.offset]
              : undefined;
            if (!next || !SPACE.test(next)) selection.insertText(" ");
          },
          { tag: HISTORY_MERGE_TAG },
        );
        onMention?.(item);
      },
    }),
    [onMention],
  );
  const { submitMode, cancelOnEscape, ...shared } = props;
  const Input = rich ? RichComposerInput : LexicalComposerInput;
  const plainOnly = rich ? {} : { submitMode, cancelOnEscape };
  return (
    <MentionLookContext.Provider value={mentionLook ?? null}>
      <Input
        {...shared}
        {...plainOnly}
        directivePluginProps={directivePluginProps}
        formatter={formatter}
        directiveChip={ComposerChip}
        data-slot="composer-input"
        data-scrolled={scrolled || undefined}
        onScroll={(event) => setScrolled(event.currentTarget.scrollTop > 0)}
        className={cn(
          "relative w-full cursor-text",
          "[&_.aui-lexical-input]:caret-primary [&_.aui-lexical-input]:whitespace-pre-wrap [&_.aui-lexical-input]:break-words [&_.aui-lexical-input]:outline-none",
          "[&_.aui-lexical-placeholder]:text-muted-foreground/60 [&_.aui-lexical-placeholder]:pointer-events-none [&_.aui-lexical-placeholder]:absolute [&_.aui-lexical-placeholder]:inset-0 [&_.aui-lexical-placeholder]:truncate [&_.aui-lexical-placeholder]:select-none",
          line
            ? "max-h-control-md text-sm [&_.aui-lexical-input]:min-h-control-md [&_.aui-lexical-input]:px-1 [&_.aui-lexical-input]:py-1.5 [&_.aui-lexical-placeholder]:px-1 [&_.aui-lexical-placeholder]:py-1.5"
            : "max-h-composer-max data-scrolled:mask-fade-top text-base [&_.aui-lexical-input]:min-h-composer [&_.aui-lexical-input]:px-2 [&_.aui-lexical-input]:py-2.5 [&_.aui-lexical-placeholder]:px-2 [&_.aui-lexical-placeholder]:py-2.5",
          className,
        )}
      >
        <ChipsPlugin formatter={formatter} kinds={kinds} editorRef={editor} />
        {children}
      </Input>
    </MentionLookContext.Provider>
  );
};
