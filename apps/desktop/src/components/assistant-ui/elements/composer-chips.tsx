import type {
  Unstable_DirectiveFormatter,
  Unstable_DirectiveSegment,
  Unstable_TriggerItem,
} from "@assistant-ui/react";
import {
  $createDirectiveNodeWithFormatter,
  $isDirectiveNode,
  type DirectiveChipProps,
  type DirectiveNode,
  LexicalComposerInput,
  type LexicalComposerInputProps,
} from "@assistant-ui/react-lexical";
import { useLexicalComposerContext } from "@lexical/react/LexicalComposerContext";
import { mergeRegister } from "@lexical/utils";
import { Globe } from "@openai/apps-sdk-ui/components/Icon";
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

import { FileTypeIcon } from "@/components/assistant-ui/elements/file-type-icon";
import {
  ComposerInlineImage,
  inlineImageLabel,
} from "@/components/assistant-ui/elements/inline-image";
import { cn } from "@/lib/utils";

/** A mention the app knows at a place in the text: its label (after `@`), kind and id. */
export type ChipMention = { label: string; type: string; id: string };

/** The known mention whose `@label` starts at `at` in `text` (`at` is past the `@`), if any. */
export type MentionMatcher = (text: string, at: number) => ChipMention | null;

/** The `[Image #n]` starting at `at` in `text`, if one does: its number and length. */
export type ImageMatcher = (text: string, at: number) => { n: number; length: number } | null;

/** Inline code and links are chips too, next to the app's mentions, and pasted images. */
const CODE = "code";
const URL_CHIP = "url";
const IMAGE_CHIP = "image";

/** The chip for image `n` pasted into the text, which stands for `[Image #n]`. */
export function imageChip(n: number): Unstable_TriggerItem {
  return { id: `${IMAGE_CHIP}:${n}`, type: IMAGE_CHIP, label: inlineImageLabel(n) };
}

const CODE_TOKEN = /(?<!`)`([^`\n]+)`(?!`)/y;
const URL_TOKEN = /https?:\/\/[^\s<>"'`]+/y;
/** Punctuation that ends a sentence after a link rather than belonging to it. */
const URL_TAIL = /[.,;:!?)\]}]+$/;
const SPACE = /\s/;
/** A character that would make `@label` part of a longer word or path. */
const WORD = /[\p{L}\p{N}_\-./]/u;

type Token = { start: number; end: number; segment: Unstable_DirectiveSegment };

const noMentions: MentionMatcher = () => null;
const noImages: ImageMatcher = () => null;

/** The chip token starting at `at` in `text`, if one does. */
function tokenAt(
  text: string,
  at: number,
  mention: MentionMatcher,
  image: ImageMatcher = noImages,
): Token | null {
  const char = text[at];
  const afterSpace = at === 0 || SPACE.test(text[at - 1] ?? "");
  if (char === "[") {
    const marker = image(text, at);
    if (marker) {
      return {
        start: at,
        end: at + marker.length,
        segment: { kind: "mention", ...imageChip(marker.n) },
      };
    }
  }
  if (char === "`") {
    CODE_TOKEN.lastIndex = at;
    const code = CODE_TOKEN.exec(text);
    if (code?.[1]) {
      const label = code[1];
      return {
        start: at,
        end: at + code[0].length,
        segment: { kind: "mention", type: CODE, label, id: `${CODE}:${label}` },
      };
    }
  }
  if (char === "h" && afterSpace) {
    URL_TOKEN.lastIndex = at;
    const url = URL_TOKEN.exec(text)?.[0].replace(URL_TAIL, "");
    if (url && url.length > "https://".length) {
      return {
        start: at,
        end: at + url.length,
        segment: { kind: "mention", type: URL_CHIP, label: url, id: `${URL_CHIP}:${url}` },
      };
    }
  }
  if (char === "@" && afterSpace) {
    const known = mention(text, at + 1);
    const end = known ? at + 1 + known.label.length : -1;
    if (known && !WORD.test(text[end] ?? "")) {
      return { start: at, end, segment: { kind: "mention", ...known } };
    }
  }
  return null;
}

/**
 * Text to segments: plain text, and the chips in it (the app's `@` mentions, `` `code` ``,
 * links and pasted images). The one parse behind a draft, a recalled prompt and a pulled queue
 * message.
 */
export function parseChips(
  text: string,
  mention: MentionMatcher,
  image: ImageMatcher = noImages,
): Unstable_DirectiveSegment[] {
  const segments: Unstable_DirectiveSegment[] = [];
  let plain = 0;
  let at = 0;
  while (at < text.length) {
    const token = tokenAt(text, at, mention, image);
    if (!token) {
      at += 1;
      continue;
    }
    if (token.start > plain) segments.push({ kind: "text", text: text.slice(plain, token.start) });
    segments.push(token.segment);
    at = token.end;
    plain = at;
  }
  if (plain < text.length) segments.push({ kind: "text", text: text.slice(plain) });
  return segments;
}

/**
 * The composer's directive formatter: a chip's text is what it stands for (`@label`,
 * `` `code` ``, the link, `[Image #n]`), so the message itself stays plain text.
 */
export function chipFormatter(
  mention: MentionMatcher,
  image: ImageMatcher = noImages,
): Unstable_DirectiveFormatter {
  return {
    serialize: (item) =>
      item.type === CODE
        ? `\`${item.label}\``
        : item.type === URL_CHIP
          ? item.label
          : item.type === IMAGE_CHIP
            ? `[${item.label}]`
            : `@${item.label}`,
    parse: (text) => parseChips(text, mention, image),
  };
}

/** The app's icon and name for a mention chip of `type` (a worker's glyph and name, a chat's title). */
export type MentionLook = (chip: DirectiveChipProps) => { icon: ReactNode; name: ReactNode } | null;

const MentionLookContext = createContext<MentionLook | null>(null);

const CHIP_ICON = "size-icon-sm mr-0.5 inline-block align-text-bottom";

function fileIcon(name: string): ReactNode {
  return <FileTypeIcon name={name} className={CHIP_ICON} />;
}

/**
 * One chip, inline in the composer's text: a blue mention, mono code pill, link or a pasted
 * image's thumbnail.
 */
const ComposerChip: FC<DirectiveChipProps> = (chip) => {
  const look = useContext(MentionLookContext);
  if (chip.directiveType === IMAGE_CHIP) {
    return <ComposerImageChip n={Number(chip.directiveId.slice(IMAGE_CHIP.length + 1))} />;
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
 * A pasted image's chip, whose × takes it out of the text as Backspace would: its image goes
 * with it, and undo brings both back.
 */
function ComposerImageChip({ n }: { n: number }) {
  const [editor] = useLexicalComposerContext();
  const remove = (event: MouseEvent<HTMLButtonElement>) => {
    const chip = event.currentTarget;
    editor.update(() => $getNearestNodeFromDOMNode(chip)?.remove());
    editor.focus();
  };
  return <ComposerInlineImage n={n} onRemove={remove} />;
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
 * The code and link tokens in `text` that are complete: a closed `` `…` ``, a link with a space
 * after it (or, pasted, at the end). With `caret`, only the one whose last character was just
 * typed there.
 */
function completedTokens(text: string, caret: number | null): Token[] {
  const tokens: Token[] = [];
  for (let at = 0; at < text.length; at += 1) {
    const token = tokenAt(text, at, noMentions);
    if (!token || token.segment.kind !== "mention") continue;
    // A link is complete once a space follows it (after any closing punctuation).
    let completedAt = token.end;
    if (token.segment.type === URL_CHIP) {
      URL_TOKEN.lastIndex = at;
      const rawEnd = at + (URL_TOKEN.exec(text)?.[0].length ?? 0);
      completedAt =
        caret === null && rawEnd === text.length
          ? rawEnd
          : SPACE.test(text[rawEnd] ?? "")
            ? rawEnd + 1
            : -1;
    }
    if (completedAt >= 0 && (caret === null || completedAt === caret)) tokens.push(token);
    at = token.end - 1;
  }
  return tokens;
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
  editorRef,
}: {
  formatter: Unstable_DirectiveFormatter;
  /** Where the input keeps the editor, for work after a menu pick. */
  editorRef: RefObject<LexicalEditor | null>;
}) {
  const [editor] = useLexicalComposerContext();
  useEffect(() => {
    editorRef.current = editor;
  }, [editor, editorRef]);
  useEffect(
    () =>
      mergeRegister(
        // What's typed is what's sent: macOS turns "..." into "…", quotes into curly ones,
        // "--" into a dash and capitalizes words as part of its text checking, which only
        // `spellcheck="false"` turns off in the webview (`autocorrect="off"` does nothing there).
        editor.registerRootListener((root) => {
          root?.setAttribute("spellcheck", "false");
          root?.setAttribute("autocorrect", "off");
          root?.setAttribute("autocapitalize", "off");
        }),
        editor.registerNodeTransform(TextNode, (node) => {
          if (editor.isComposing() || !node.isSimpleText()) return;
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
          const token = completedTokens(node.getTextContent(), pasted ? null : caret)[0];
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
      ),
    [editor, formatter],
  );
  return null;
}

/**
 * assistant-ui's Lexical composer input with inline chips: a mention shows its icon
 * and blue name, `` `code` `` a mono pill, a link a globe and blue text, while the composer's
 * text stays plain. It grows with its text up to `max-h-composer-max`, then scrolls, the top
 * line fading under the edge. `children` are further Lexical plugins.
 */
export const ChipComposerInput: FC<
  Omit<LexicalComposerInputProps, "formatter" | "directiveChip" | "directivePluginProps"> & {
    formatter: Unstable_DirectiveFormatter;
    mentionLook?: MentionLook;
    /** Hears each item picked from a menu (it's in the text as a chip by then). */
    onMention?: (item: Unstable_TriggerItem) => void;
    /** One line (under an action card): it doesn't grow, it scrolls. */
    line?: boolean;
  }
> = ({ formatter, mentionLook, onMention, line = false, className, children, ...props }) => {
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
  return (
    <MentionLookContext.Provider value={mentionLook ?? null}>
      <LexicalComposerInput
        {...props}
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
        <ChipsPlugin formatter={formatter} editorRef={editor} />
        {children}
      </LexicalComposerInput>
    </MentionLookContext.Provider>
  );
};
