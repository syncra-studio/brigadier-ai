/**
 * While a reply streams, each word fades in on its own as it shows (`.aui-fade` in globals.css).
 * A rehype pass wraps every word of the text in a span. A span's key is its place among its
 * siblings, so a word keeps its element (and its fade) while it grows and more words arrive
 * after it. A word shown for longer than the fade loses the class, so an element made again
 * for it doesn't replay the fade. Inline code, rules, table rows, quotes, list items and images
 * fade in as units, as slowly as a word; list markers fade with their words.
 */

type HastNode = {
  type: string;
  tagName?: string;
  value?: string;
  properties?: Record<string, unknown>;
  children?: HastNode[];
  position?: { start?: { offset?: number | undefined } } | undefined;
};

/** How long a word takes to fade in; keep in step with `.aui-fade` in globals.css. */
const FADE_MS = 700;

/** The classes an element that shows fades in with, as a unit. */
const UNITS: Record<string, string[]> = {
  code: ["aui-fade"],
  hr: ["aui-fade-block"],
  tr: ["aui-fade-block"],
  blockquote: ["aui-fade-block"],
  li: ["aui-fade-block", "aui-fade-marker"],
  img: ["aui-fade-block"],
};

const ASCII = /^\p{ASCII}*$/u;
const ASCII_PIECE = /[0-9A-Za-z]+|[^0-9A-Za-z]+/g;
const segmenter =
  typeof Intl.Segmenter === "function"
    ? new Intl.Segmenter(undefined, { granularity: "word" })
    : null;

/**
 * The text's words, each with the spaces and punctuation after it, at their offsets in the
 * text. ASCII words are runs of letters and digits; other scripts split as the locale does.
 */
function words(text: string): { at: number; text: string }[] {
  const pieces: { at: number; text: string; word: boolean }[] = [];
  if (ASCII.test(text) || !segmenter) {
    for (const match of text.matchAll(ASCII_PIECE)) {
      pieces.push({ at: match.index, text: match[0], word: /[0-9A-Za-z]/.test(match[0][0] ?? "") });
    }
  } else {
    for (const piece of segmenter.segment(text)) {
      pieces.push({ at: piece.index, text: piece.segment, word: piece.isWordLike === true });
    }
  }
  const out: { at: number; text: string }[] = [];
  for (const piece of pieces) {
    const previous = out.at(-1);
    if (!piece.word && previous) previous.text += piece.text;
    else out.push({ at: piece.at, text: piece.text });
  }
  return out;
}

function withClasses(node: HastNode, classes: string[]): void {
  const current = node.properties?.["className"];
  const list = Array.isArray(current) ? current : typeof current === "string" ? [current] : [];
  node.properties = { ...node.properties, className: [...list, ...classes] };
}

/**
 * A rehype plugin for one streaming text: it remembers when each word first showed, by its
 * offset in the Markdown. Whatever shows in the first pass counts as long shown, so a text that
 * mounts again part way through doesn't fade in whole.
 */
export function createWordFade(): () => (tree: HastNode) => void {
  const shownAt = new Map<string, number>();
  let first = true;
  return () => (tree) => {
    const now = performance.now();
    const settled = first;
    first = false;
    const fading = (id: string) => {
      let at = shownAt.get(id);
      if (at === undefined) {
        at = settled ? Number.NEGATIVE_INFINITY : now;
        shownAt.set(id, at);
      }
      return now - at < FADE_MS;
    };

    const walk = (node: HastNode) => {
      const children = node.children;
      if (!children) return;
      const out: HastNode[] = [];
      for (const child of children) {
        const offset = child.position?.start?.offset;
        if (child.type === "text") {
          // Whitespace between blocks (and between table cells) stays as it is.
          if (!child.value?.trim()) {
            out.push(child);
            continue;
          }
          for (const word of words(child.value)) {
            out.push({
              type: "element",
              tagName: "span",
              properties:
                offset !== undefined && fading(`${offset + word.at}`)
                  ? { className: ["aui-fade"] }
                  : {},
              children: [{ type: "text", value: word.text }],
            });
          }
          continue;
        }
        if (child.type === "element" && child.tagName !== "pre") {
          const classes = UNITS[child.tagName ?? ""];
          if (classes && offset !== undefined && fading(`${child.tagName}:${offset}`)) {
            withClasses(child, classes);
          }
          // Inline code fades as a unit; its text stays whole.
          if (child.tagName !== "code") walk(child);
        }
        out.push(child);
      }
      node.children = out;
    };
    walk(tree);
  };
}
