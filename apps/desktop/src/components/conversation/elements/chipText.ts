import type {
  Unstable_DirectiveFormatter,
  Unstable_DirectiveSegment,
  Unstable_TriggerItem,
} from "@assistant-ui/react";

/**
 * The composer's chips as text: which spans of a message are chips (the app's `@` mentions,
 * pasted images, `` `code` `` and links) and what each one's text is. No React here, so the
 * markdown round trip can be tested on its own.
 */

/** A mention the app knows at a place in the text: its label (after `@`), kind and id. */
export type ChipMention = { label: string; type: string; id: string };

/** The known mention whose `@label` starts at `at` in `text` (`at` is past the `@`), if any. */
export type MentionMatcher = (text: string, at: number) => ChipMention | null;

/**
 * The `[Image #n]` (or `[Pasted text #n]`, kind `paste`) starting at `at` in `text`, if one
 * does: its number and length.
 */
export type ImageMatcher = (
  text: string,
  at: number,
) => { n: number; length: number; kind?: "image" | "paste" } | null;

/**
 * Which text-made chips a composer has: `` `code` `` (the plain field's; the rich one has a
 * code mark instead) and links. Mentions and pasted images are always chips.
 */
export type ChipKinds = { code: boolean; links: boolean };

export const ALL_CHIPS: ChipKinds = { code: true, links: true };

/** Inline code and links are chips too, next to the app's mentions, and pasted images. */
export const CODE = "code";
export const URL_CHIP = "url";
export const IMAGE_CHIP = "image";
export const PASTE_CHIP = "paste";

/** What a pasted image is called where it sits in the text. */
export function inlineImageLabel(n: number): string {
  return `Image #${n}`;
}

/** The chip for image `n` pasted into the text, which stands for `[Image #n]`. */
export function imageChip(n: number): Unstable_TriggerItem {
  return { id: `${IMAGE_CHIP}:${n}`, type: IMAGE_CHIP, label: inlineImageLabel(n) };
}

/** What a text pasted as a chip is called where it sits in the text. */
export function pastedTextLabel(n: number): string {
  return `Pasted text #${n}`;
}

/** The chip for text `n` pasted into the text, which stands for `[Pasted text #n]`. */
export function pasteChip(n: number): Unstable_TriggerItem {
  return { id: `${PASTE_CHIP}:${n}`, type: PASTE_CHIP, label: pastedTextLabel(n) };
}

const CODE_TOKEN = /(?<!`)`([^`\n]+)`(?!`)/y;
export const URL_TOKEN = /https?:\/\/[^\s<>"'`]+/y;
/**
 * Punctuation that ends a sentence after a link rather than belonging to it, and Markdown's
 * emphasis markers closing around it (`**see https://x.com**`).
 */
const URL_TAIL = /[.,;:!?)\]}*_~]+$/;
export const SPACE = /\s/;
/** A character that would make `@label` part of a longer word or path. */
const WORD = /[\p{L}\p{N}_\-./]/u;

export type Token = { start: number; end: number; segment: Unstable_DirectiveSegment };

export const noMentions: MentionMatcher = () => null;
const noImages: ImageMatcher = () => null;

/** The chip token starting at `at` in `text`, if one does. */
export function tokenAt(
  text: string,
  at: number,
  mention: MentionMatcher,
  image: ImageMatcher = noImages,
  kinds: ChipKinds = ALL_CHIPS,
): Token | null {
  const char = text[at];
  const afterSpace = at === 0 || SPACE.test(text[at - 1] ?? "");
  if (char === "[") {
    const marker = image(text, at);
    if (marker) {
      return {
        start: at,
        end: at + marker.length,
        segment: { kind: "mention", ...(marker.kind === "paste" ? pasteChip(marker.n) : imageChip(marker.n)) },
      };
    }
  }
  if (char === "`" && kinds.code) {
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
  if (char === "h" && afterSpace && kinds.links) {
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
  kinds: ChipKinds = ALL_CHIPS,
): Unstable_DirectiveSegment[] {
  const segments: Unstable_DirectiveSegment[] = [];
  let plain = 0;
  let at = 0;
  while (at < text.length) {
    const token = tokenAt(text, at, mention, image, kinds);
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
 * `` `code` ``, the link, `[Image #n]`, `[Pasted text #n]`), so the message itself stays text.
 */
export function chipFormatter(
  mention: MentionMatcher,
  image: ImageMatcher = noImages,
  kinds: ChipKinds = ALL_CHIPS,
): Unstable_DirectiveFormatter {
  return {
    serialize: (item) =>
      item.type === CODE
        ? `\`${item.label}\``
        : item.type === URL_CHIP
          ? item.label
          : item.type === IMAGE_CHIP || item.type === PASTE_CHIP
            ? `[${item.label}]`
            : `@${item.label}`,
    parse: (text) => parseChips(text, mention, image, kinds),
  };
}

/**
 * The code and link tokens in `text` that are complete: a closed `` `…` ``, a link with a space
 * after it (or, pasted, at the end). With `caret`, only the one whose last character was just
 * typed there.
 */
export function completedTokens(text: string, caret: number | null, kinds: ChipKinds = ALL_CHIPS): Token[] {
  const tokens: Token[] = [];
  for (let at = 0; at < text.length; at += 1) {
    const token = tokenAt(text, at, noMentions, noImages, kinds);
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
