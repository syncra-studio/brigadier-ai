import { create } from "zustand";
import { persist } from "zustand/middleware";

/** Plan shows while a plan is open in it (`showPlanDoc`). */
export type RightSidebarTab = "workers" | "plan";

/** One open choice for the app, with each session remembering its front tab. */
type RightSidebarState = {
  open: boolean;
  tabs: Record<string, RightSidebarTab>;
};

export const useRightSidebarState = create<RightSidebarState>()(
  persist((): RightSidebarState => ({ open: false, tabs: {} }), {
    name: "brigadier.rightSidebar",
    version: 1,
    merge: (persisted, current) => {
      const saved = persisted as Partial<RightSidebarState> | null;
      return { ...current, open: Boolean(saved?.open) && Object.values(saved?.tabs ?? {}).some((tab) => tab === "workers"), tabs: Object.fromEntries(
        Object.entries(saved?.tabs && typeof saved.tabs === "object" ? saved.tabs : {})
          .filter(([, tab]) => isRightSidebarTab(tab)),
      ) };
    },
  }),
);

export function setRightSidebarOpen(next: boolean | ((open: boolean) => boolean)): void {
  useRightSidebarState.setState(({ open }) => ({
    open: typeof next === "function" ? next(open) : next,
  }));
}

export function selectRightSidebarTab(id: string, tab: RightSidebarTab): void {
  useRightSidebarState.setState(({ tabs }) => ({ tabs: { ...tabs, [id]: tab } }));
}

/** A tab a session comes back to at launch: not Plan, whose open plan isn't kept. */
export function isRightSidebarTab(tab: string): tab is RightSidebarTab {
  return tab === "workers";
}

/** With an expanded left sidebar, the right one folds first as the window narrows. */
export function rightSidebarFolds(windowWidth: number, leftOpen: boolean, leftWidth: number, narrowAt: number): boolean {
  return windowWidth < narrowAt + (leftOpen ? leftWidth : 0);
}

/** Deleted sessions leave no preference behind; failed deletions keep theirs. */
export function forgetRightSidebarTabs(ids: readonly string[]): void {
  const tabs = { ...useRightSidebarState.getState().tabs };
  for (const id of ids) delete tabs[id];
  useRightSidebarState.setState({ tabs });
}

/** A refreshed catalog is authoritative, including sessions deleted while the app was away. */
export function pruneRightSidebarTabs(ids: readonly string[]): void {
  const kept = new Set(ids);
  const gone = Object.keys(useRightSidebarState.getState().tabs).filter((id) => !kept.has(id));
  if (gone.length) forgetRightSidebarTabs(gone);
}
