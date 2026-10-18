/**
 * A research answer's sources (Q79): numbered markers in its text ([1], [2]) and one `### Sources`
 * list at its end, which the thread folds under "Sources · N" and whose markers link to their
 * source.
 */

/** One source of the list: its number as the markers say it, its words and its address. */
export type AnswerSource = { n: number; title: string; url: string };

/** The `### Sources` heading as a model writes it too: `## Sources`, `**Sources:**`, `Sources:`. */
const HEADING = /^(?:#{2,4}[ \t]+)?(?:\*\*)?Sources:?(?:\*\*)?:?[ \t]*$/im;
/** A list item: `1. …`, `- …`, `* …`, or `[1] …`. */
const ITEM = /^[ \t]*(?:(\d+)[.)]|[-*+]|\[(\d+)\])[ \t]+(.+)$/;
/**
 * A Markdown link in an item, or a bare address. A link's address keeps its balanced
 * parentheses, as in `https://en.wikipedia.org/wiki/Python_(programming_language)`.
 */
const LINK = /\[([^\]]+)\]\(<([^>\n]+)>(?:[ \t]+"[^"]*")?\)|\[([^\]]+)\]\(((?:[^\s()]|\([^\s()]*\))+)(?:[ \t]+"[^"]*")?\)|(https?:\/\/\S+)/;

/** A bare address without the sentence's punctuation after it: `.`/`,`/`;` and a `)` it doesn't open. */
function trimBare(url: string): string {
  let trimmed = url.replace(/[.,;:]+$/, "");
  while (trimmed.endsWith(")") && trimmed.split(")").length > trimmed.split("(").length) {
    trimmed = trimmed.slice(0, -1).replace(/[.,;:]+$/, "");
  }
  return trimmed;
}

/**
 * The answer split at its trailing sources list: the text above it and each source, numbered as
 * listed (an item's own number wins). Null when there is no such list, or a heading has nothing
 * but sources-shaped lines after it.
 */
export function splitSources(text: string): { head: string; sources: AnswerSource[] } | null {
  const at = text.search(HEADING);
  if (at < 0) return null;
  const end = text.indexOf("\n", at);
  if (end < 0) return null;
  const lines = text.slice(end + 1).split("\n").filter((line) => line.trim() !== "");
  const sources: AnswerSource[] = [];
  for (const line of lines) {
    const item = ITEM.exec(line);
    if (!item) return null;
    const words = item[3]!;
    const link = LINK.exec(words);
    if (!link) return null;
    const url = link[2] ?? link[4] ?? trimBare(link[5]!);
    const title = link[1] ?? link[3] ?? (words.replace(link[0], "").replace(/[\s–—:-]+$/, "").trim() || url);
    sources.push({ n: Number(item[1] ?? item[2] ?? sources.length + 1), title, url });
  }
  if (sources.length === 0) return null;
  return { head: text.slice(0, at).trimEnd(), sources };
}

/**
 * `text` with each marker that names a listed source ([1], [2, 3]) as a `brigadier-cite:n` link,
 * outside code. A marker followed by `(` or `:` is Markdown of its own and stays as written.
 */
export function linkMarkers(text: string, sources: readonly AnswerSource[]): string {
  const known = new Set(sources.map((source) => source.n));
  // Fenced blocks and inline code keep their brackets: only the odd pieces of a split on them are prose.
  return text
    .split(/(```[\s\S]*?(?:```|$)|`[^`\n]*`)/)
    .map((piece, index) =>
      index % 2 === 1
        ? piece
        : piece.replace(/\[(\d+(?:[ \t]*,[ \t]*\d+)*)\](?![(:])/g, (whole, numbers: string) => {
            const each = numbers.split(",").map((n) => Number(n.trim()));
            if (!each.every((n) => known.has(n))) return whole;
            return each.map((n) => `[${n}](brigadier-cite:${n})`).join("");
          }),
    )
    .join("");
}
