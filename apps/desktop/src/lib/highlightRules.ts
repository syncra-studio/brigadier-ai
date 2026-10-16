/**
 * The thread's `::highlight()` colours, on the page only while their marks are. A `::highlight()`
 * rule in a stylesheet has the browser work out highlight styles for every element on every style
 * pass: the three of them cost the first paint about 60 ms of style work (GPU off, CPU ×6), for
 * marks that are rarely there (Find in the chat, text added to the chat).
 */
const COLOURS = {
  "thread-annotation": "var(--annotation-highlight)",
  "thread-find": "var(--find-match)",
  "thread-find-active": "var(--find-match-active)",
} as const;

export type HighlightName = keyof typeof COLOURS;

const shown = new Set<HighlightName>();
let sheet: HTMLStyleElement | null = null;

/** Puts the colour of the highlight `name` on the page, or takes it off. */
export function showHighlight(name: HighlightName, on: boolean): void {
  if (on === shown.has(name)) return;
  if (on) shown.add(name);
  else shown.delete(name);
  if (!sheet) {
    sheet = document.createElement("style");
    sheet.dataset["highlights"] = "";
    document.head.append(sheet);
  }
  sheet.textContent = [...shown].map((each) => `::highlight(${each}) { background-color: ${COLOURS[each]}; }`).join("\n");
}
