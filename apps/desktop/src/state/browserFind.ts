import { create } from "zustand";

import { browserFind } from "@/ipc/client";

/**
 * Each Browser tab's find bar: open or not, what it looks for, and what its page found. It
 * lasts while the tab does, so switching tabs and back finds it as it was.
 */

export type FindState = {
  open: boolean;
  query: string;
  /** The page has the text; its match `current` (from 1, 0 unknown) of `total` is selected. */
  found: boolean;
  current: number;
  total: number;
  /** The query's first answer hasn't come back yet. */
  searching: boolean;
};

export const NO_FIND: FindState = { open: false, query: "", found: false, current: 0, total: 0, searching: false };

export const useBrowserFind = create<{ tabs: Record<string, FindState> }>(() => ({ tabs: {} }));

function patch(id: string, change: Partial<FindState>): void {
  useBrowserFind.setState(({ tabs }) => ({ tabs: { ...tabs, [id]: { ...(tabs[id] ?? NO_FIND), ...change } } }));
}

export function findState(id: string): FindState {
  return useBrowserFind.getState().tabs[id] ?? NO_FIND;
}

/** The latest search asked of each tab: an older one's answer, come late, is dropped. */
const asked = new Map<string, number>();

/**
 * Looks for `query` in the tab's page: `"type"` as it is typed (the match selected stays while it
 * still matches), `"next"` and `"previous"` from the match selected on.
 */
export async function findInPage(id: string, query: string, step: "type" | "next" | "previous"): Promise<void> {
  const turn = (asked.get(id) ?? 0) + 1;
  asked.set(id, turn);
  if (!query) {
    patch(id, { query, found: false, current: 0, total: 0, searching: false });
    await browserFind(id, "", "type").catch(() => {});
    return;
  }
  patch(id, step === "type" ? { query, searching: true } : { query });
  try {
    const found = await browserFind(id, query, step);
    if (asked.get(id) !== turn) return;
    patch(id, { ...found, searching: false });
  } catch {
    if (asked.get(id) !== turn) return;
    patch(id, { found: false, current: 0, total: 0, searching: false });
  }
}

export function openFind(id: string): void {
  patch(id, { open: true });
}

/** Closes the tab's find bar and lets go of its match; the query stays for the next time. */
export function closeFind(id: string): void {
  asked.set(id, (asked.get(id) ?? 0) + 1);
  patch(id, { open: false, found: false, current: 0, total: 0, searching: false });
  browserFind(id, "", "type").catch(() => {});
}

/** The tab closed: its find bar goes with it. */
export function forgetFind(id: string): void {
  asked.delete(id);
  useBrowserFind.setState(({ tabs }) => {
    const { [id]: _gone, ...rest } = tabs;
    return { tabs: rest };
  });
}

/** What the find bar says of its search: "3 of 12", "No results", or nothing yet. */
export function findLabel({ query, found, current, total, searching }: FindState): string {
  if (!query || searching) return "";
  if (!found) return "No results";
  if (total === 0) return "Found";
  return `${Math.max(1, current)} of ${total >= 1000 ? "1000+" : total}`;
}
