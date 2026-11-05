import { create } from "zustand";

import { useApp, type Selection } from "@/state/store";

/** Home: what Back ends on, and what shows in place of a place that is gone. */
export const HOME: Selection = { type: "draft", kind: "chat" };

/**
 * What Forward can redo: the places Back left, latest last. Any other navigation clears it. It
 * lives in memory and starts afresh on each launch.
 */
export const useHistory = create<{ forward: Selection[] }>(() => ({ forward: [] }));

/**
 * Where Back goes from `selection`, by where it sits and never by how it was reached: a Settings
 * detail to its page, a page to General, General to what Settings was opened from
 * (`beforeSettings`), a conversation or a project's draft to Home. Home has nowhere to go.
 */
export function levelUp(selection: Selection, beforeSettings: Selection): Selection | null {
  switch (selection.type) {
    case "none":
      return null;
    case "conversation":
      return HOME;
    case "draft":
      return selection.kind === "chat" ? null : HOME;
    case "settings":
      if (selection.detail) return { type: "settings", page: selection.page };
      return selection.page === "general" ? beforeSettings : { type: "settings", page: "general" };
  }
}

/** Whether two selections show the same place, including pages inside Settings pages. */
export function samePlace(a: Selection, b: Selection): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/** Whether Back and Forward have somewhere to go. */
export function useCanStep(): { back: boolean; forward: boolean } {
  const back = useApp((s) => levelUp(s.selection, HOME) !== null);
  const forward = useHistory((s) => s.forward.length > 0);
  return { back, forward };
}
