/*
 * Find's words for a reply the thread hasn't mounted (app/conversation/ThreadFind.tsx): its
 * Markdown as the reply draws it. Its own module, so it is tested without the bar.
 */

/** A code fence's line: the fence goes, the lines between show as written. */
const FENCE = /^\s{0,3}```/;
/** Inline code: its backticks go, what it says shows as written. */
const INLINE_CODE = /(`+)([\s\S]*?[^`])\1(?!`)/;

/** Markdown's marks prose shows without: links' targets, emphasis. */
function unmarkedProse(text: string): string {
  return text.replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1").replace(/\*\*|__|~~|`/g, "");
}

/**
 * Markdown's marks, which a message's text drawn shows without: fences, heads, quotes, list marks,
 * links' targets, emphasis. Code, fenced or inline, keeps what it says (`__init__`).
 */
export function unmarked(text: string): string {
  let fenced = false;
  return text
    .split("\n")
    .map((line) => {
      if (FENCE.test(line)) {
        fenced = !fenced;
        return "";
      }
      if (fenced) return line;
      const rest = line.replace(/^\s{0,3}(?:#{1,6}\s+|>\s?|[-*+]\s+|\d+[.)]\s+)/, "");
      // Prose and inline code by turns: [prose, ticks, code, prose, …].
      return rest
        .split(new RegExp(INLINE_CODE.source, "g"))
        .map((part, index) => (index % 3 === 0 ? unmarkedProse(part) : index % 3 === 2 ? part : ""))
        .join("");
    })
    .join("\n");
}
