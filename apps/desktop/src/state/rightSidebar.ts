import { create } from "zustand";
import { persist } from "zustand/middleware";

export type RightSidebarTab = "files" | "source" | "workers";

/** One open choice for the app, with each session remembering its front tab. */
type RightSidebarState = {
  open: boolean;
  tabs: Record<string, RightSidebarTab>;
};

export const useRightSidebarState = create<RightSidebarState>()(
  persist((): RightSidebarState => ({ open: false, tabs: {} }), {
    name: "brigadier.rightSidebar",
    version: 1,
  }),
);

export function setRightSidebarOpen(next: boolean | ((open: boolean) => boolean)): void {
  useRightSidebarState.setState(({ open }) => ({
    open: typeof next === "function" ? next(open) : next,
  }));
}

export function selectRightSidebarTab(id: string, tab: RightSidebarTab): void {
  useRightSidebarState.setState(({ tabs }) => {
    if (tab === "files") {
      const { [id]: _default, ...rest } = tabs;
      return { tabs: rest };
    }
    return { tabs: { ...tabs, [id]: tab } };
  });
}

export function isRightSidebarTab(tab: string): tab is RightSidebarTab {
  return tab === "files" || tab === "source" || tab === "workers";
}

export function isRightSidebarKey(event: KeyboardEvent, mac: boolean): boolean {
  return (mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey)
    && event.altKey && !event.shiftKey && !event.isComposing && !event.getModifierState?.("AltGraph") && event.code === "KeyB";
}

/** With an expanded left sidebar, the right one folds first as the window narrows. */
export function rightSidebarFolds(windowWidth: number, leftOpen: boolean, leftWidth: number, narrowAt: number): boolean {
  return windowWidth < narrowAt + (leftOpen ? leftWidth : 0);
}

export function rightSidebarToggleLabel(open: boolean): string {
  return open ? "Hide right sidebar" : "Show right sidebar";
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
