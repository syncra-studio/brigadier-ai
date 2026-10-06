import { create } from "zustand";

import { useApp, type Selection } from "@/state/store";

/** How many places Back can go through. */
const MAX_ENTRIES = 100;

/**
 * How a selection enters the history: a new entry (dropping what Forward would have gone to),
 * in place of the current one, or not at all (Back and Forward themselves).
 */
export type HistoryMode = "push" | "replace" | "none";

/**
 * Where the window has been, for Back and Forward: the places shown, oldest first, and which
 * one shows. It lives in memory and starts afresh on each launch.
 */
export const useHistory = create<{ entries: Selection[]; index: number }>(() => ({
  entries: [],
  index: -1,
}));

/** A place as history tells places apart: Settings is one place whatever its page. */
function placeKey(selection: Selection): string | null {
  switch (selection.type) {
    case "conversation":
      return `conversation:${selection.id}`;
    case "draft":
      return selection.kind === "chat" ? "draft:chat" : `draft:session:${selection.projectId}`;
    case "settings":
      return "settings";
    case "none":
      return null;
  }
}

/**
 * Notes that `selection` is about to show. A push onto the place already showing (another
 * Settings page, the same conversation again) updates that entry instead.
 */
export function recordPlace(selection: Selection, mode: HistoryMode): void {
  if (mode === "none" || selection.type === "none") return;
  let { entries, index } = useHistory.getState();
  if (entries.length === 0) {
    // The place shown at launch is the first entry.
    const { selection: shown } = useApp.getState();
    entries = shown.type === "none" ? [] : [shown];
    index = entries.length - 1;
  }
  const current = entries[index];
  if (current && (mode === "replace" || placeKey(current) === placeKey(selection))) {
    const next = entries.slice();
    next[index] = selection;
    useHistory.setState({ entries: next, index });
    return;
  }
  const next = [...entries.slice(0, index + 1), selection].slice(-MAX_ENTRIES);
  useHistory.setState({ entries: next, index: next.length - 1 });
}

/** Moves `delta` entries along, returning the entry arrived at (null at either end). */
export function stepPlace(delta: -1 | 1): Selection | null {
  const { entries, index } = useHistory.getState();
  const target = entries[index + delta];
  if (!target) return null;
  useHistory.setState({ index: index + delta });
  return target;
}

/** Puts `selection` in place of the entry showing (a place found gone on arrival). */
export function replacePlace(selection: Selection): void {
  const { entries, index } = useHistory.getState();
  if (!entries[index]) return;
  const next = entries.slice();
  next[index] = selection;
  useHistory.setState({ entries: next });
}

/** Whether Back and Forward have somewhere to go. */
export function useCanStep(): { back: boolean; forward: boolean } {
  const back = useHistory((s) => s.index > 0);
  const forward = useHistory((s) => s.index < s.entries.length - 1);
  return { back, forward };
}
