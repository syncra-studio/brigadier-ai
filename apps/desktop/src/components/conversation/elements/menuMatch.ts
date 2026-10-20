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
 * `/` opens the commands menu as the message's first character, after any spaces or blank
 * lines; the menu closes at a space after it.
 */
export const slashTrigger: Unstable_TriggerMatcher = (text, char, cursor) => {
  const lead = /^\s*/u.exec(text)?.[0].length ?? 0;
  if (!text.startsWith(char, lead)) return null;
  const start = lead + char.length;
  if (cursor < start) return null;
  const query = text.slice(start, cursor);
  if (/\s/u.test(query)) return null;
  return { query, offset: lead, endOffset: cursor };
};
