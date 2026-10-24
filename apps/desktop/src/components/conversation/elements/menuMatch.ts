import type { Unstable_TriggerMatcher } from "@assistant-ui/react";

/**
 * Where `query` matches `text`, as the `/` and `@` menus match: at its start, anywhere
 * in it, or letter by letter in order. The positions of the matched letters, and how good the
 * match is (0 = start, 1 = anywhere, 2 = letter by letter); null when it doesn't match.
 */
export function fuzzyMatch(text: string, query: string): { rank: 0 | 1 | 2; at: number[] } | null {
  const lower = text.toLowerCase();
  const want = query.toLowerCase();
  const run = (from: number) => Array.from(want, (_, index) => from + index);
  if (lower.startsWith(want)) return { rank: 0, at: run(0) };
  const inside = lower.indexOf(want);
  if (inside >= 0) return { rank: 1, at: run(inside) };
  const at: number[] = [];
  let from = 0;
  for (const letter of want) {
    const found = lower.indexOf(letter, from);
    if (found < 0) return null;
    at.push(found);
    from = found + 1;
  }
  return { rank: 2, at };
}

/**
 * `/` opens the commands menu where a word starts: at the start of the message or of a line,
 * or after a space, anywhere in the text. Never inside a word (a path like `src/a`, a link like
 * `http://x/y`); the menu closes at a space after it.
 */
export const slashTrigger: Unstable_TriggerMatcher = (text, char, cursor) => {
  // The word the caret is in, from the space or line start before it.
  const word = /\S*$/u.exec(text.slice(0, cursor))?.[0] ?? "";
  if (!word.startsWith(char)) return null;
  return { query: word.slice(char.length), offset: cursor - word.length, endOffset: cursor };
};
