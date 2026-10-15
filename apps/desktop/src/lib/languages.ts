/**
 * What highlighting needs to know before shiki loads (`@/lib/highlight`, on demand): a file's
 * grammar and the size past which text stays plain. Light, so the thread's startup code can use
 * it without pulling the highlighter into the first paint.
 */

export type Token = { content: string; color: string | undefined; italic: boolean };

/** Text longer than this shows without colours, which would take too long to work out. */
export const HIGHLIGHT_CHARS = 300_000;

/** The grammar for a file: its extension, or its name for the ones known without one. */
export function languageOf(path: string): string {
  const name = path.slice(path.lastIndexOf("/") + 1).toLowerCase();
  if (name === "dockerfile" || name === "makefile") return name;
  const dot = name.lastIndexOf(".");
  return dot < 0 ? "" : name.slice(dot + 1);
}
