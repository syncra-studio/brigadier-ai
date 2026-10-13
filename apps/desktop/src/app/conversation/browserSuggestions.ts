import { browserAddress, webAddress } from "./browserAddress";

import type { BrowserVisit } from "@/ipc/generated";

/**
 * What the Browser tab's address field suggests while it is typed in: going to the address or
 * searching the web for the words first, then the session's other Browser tabs that match (to
 * switch to), then the pages of the history that match.
 */
export type Suggestion =
  | { kind: "go"; url: string; search: boolean; text: string }
  | { kind: "tab"; id: string; url: string; title: string }
  | { kind: "visit"; id: number; url: string; title: string };

/** A Browser tab of the session, as the address field may switch to it. */
export type OpenTab = { id: string; url: string; title: string };

/** At most so many open tabs, and suggestions in all. */
const TABS = 3;
const ALL = 8;

const web = (url: string) => /^https?:\/\//i.test(url);

/**
 * The suggestions for `typed`, from the session's Browser tabs `tabs` (the one typed in,
 * `self`, left out) and history `visits` (newest first): none for nothing typed.
 */
export function addressSuggestions(
  typed: string,
  tabs: readonly OpenTab[],
  visits: readonly BrowserVisit[],
  self: string,
): Suggestion[] {
  const text = typed.trim();
  const to = browserAddress(text);
  if (!to) return [];
  const words = text.toLowerCase();
  const matches = (url: string, title: string) =>
    url.toLowerCase().includes(words) || title.toLowerCase().includes(words);
  const found: Suggestion[] = [{ kind: "go", url: to, search: webAddress(text) === null, text }];
  const shown = new Set<string>([to]);
  let switches = 0;
  for (const tab of tabs) {
    if (switches === TABS) break;
    if (tab.id === self || !web(tab.url) || !matches(tab.url, tab.title)) continue;
    switches++;
    found.push({ kind: "tab", id: tab.id, url: tab.url, title: tab.title });
    shown.add(tab.url);
  }
  for (const visit of visits) {
    if (found.length >= ALL) break;
    if (!web(visit.url) || shown.has(visit.url) || !matches(visit.url, visit.title)) continue;
    found.push({ kind: "visit", id: visit.id, url: visit.url, title: visit.title });
    shown.add(visit.url);
  }
  return found;
}

/** The row `step` (1 down, -1 up) from row `at` of `count`; -1 is the field itself. */
export function stepSuggestion(at: number, step: 1 | -1, count: number): number {
  if (count === 0) return -1;
  const next = at + step;
  if (next >= count) return -1;
  if (next < -1) return count - 1;
  return next;
}
