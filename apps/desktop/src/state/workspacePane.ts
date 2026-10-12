import { create } from "zustand";
import { persist } from "zustand/middleware";

export type WorkspacePaneTab = "files" | "source";
export type WorkspacePaneState = { open: boolean; tabs: Record<string, WorkspacePaneTab> };
export const WORKSPACE_PANE_KEY = "brigadier.workspacePane";
export const WORKSPACE_PANE_WIDTH_KEY = "brigadier.workspacePaneWidth";

export function toolbarToggles(kind: "chat" | "session"): readonly ("context" | "files" | "source" | "terminal")[] {
  return kind === "session" ? ["context", "files", "source", "terminal"] : ["context", "terminal"];
}

export function isWorkspacePaneTab(tab: unknown): tab is WorkspacePaneTab {
  return tab === "files" || tab === "source";
}

export function workspacePaneState(saved: unknown): WorkspacePaneState {
  const value = saved as Partial<WorkspacePaneState> | null;
  return { open: Boolean(value?.open), tabs: Object.fromEntries(
    Object.entries(value?.tabs && typeof value.tabs === "object" ? value.tabs : {})
      .filter(([, tab]) => isWorkspacePaneTab(tab)),
  ) };
}

/** Old Files was the implicit default. Worker, Plan and preview tabs don't reopen the pane. */
export function legacyWorkspacePane(saved: unknown): WorkspacePaneState {
  const value = saved as { open?: unknown; tabs?: Record<string, unknown> } | null;
  const tabs = value?.tabs && typeof value.tabs === "object" ? value.tabs : {};
  return { ...workspacePaneState(value), open: Boolean(value?.open)
    && (Object.keys(tabs).length === 0 || Object.values(tabs).some(isWorkspacePaneTab)) };
}

function initialWorkspacePane(): WorkspacePaneState {
  try {
    if (localStorage.getItem(WORKSPACE_PANE_KEY)) return { open: false, tabs: {} };
    const legacy = JSON.parse(localStorage.getItem("brigadier.rightSidebar") ?? "null") as { state?: unknown } | null;
    const state = legacyWorkspacePane(legacy?.state);
    localStorage.setItem(WORKSPACE_PANE_KEY, JSON.stringify({ state, version: 1 }));
    const width = localStorage.getItem("brigadier.rightSidebarWidth");
    if (width && !localStorage.getItem(WORKSPACE_PANE_WIDTH_KEY)) localStorage.setItem(WORKSPACE_PANE_WIDTH_KEY, width);
    return state;
  } catch {
    return { open: false, tabs: {} };
  }
}

export const useWorkspacePaneState = create<WorkspacePaneState>()(
  persist(initialWorkspacePane, {
    name: WORKSPACE_PANE_KEY, version: 1,
    merge: (saved, current) => ({ ...current, ...workspacePaneState(saved) }),
  }),
);

export function toggledWorkspacePane(current: WorkspacePaneState, id: string, tab: WorkspacePaneTab): WorkspacePaneState {
  return { open: !current.open || (current.tabs[id] ?? "files") !== tab, tabs: { ...current.tabs, [id]: tab } };
}

export function toggleWorkspacePane(id: string, tab: WorkspacePaneTab): void {
  useWorkspacePaneState.setState((current) => toggledWorkspacePane(current, id, tab));
}

export function openWorkspacePane(id: string, tab: WorkspacePaneTab): void {
  useWorkspacePaneState.setState(({ tabs }) => ({ open: true, tabs: { ...tabs, [id]: tab } }));
}

export function setWorkspacePaneOpen(next: boolean | ((open: boolean) => boolean)): void {
  useWorkspacePaneState.setState(({ open }) => ({ open: typeof next === "function" ? next(open) : next }));
}

export function isFilesSearchKey(event: KeyboardEvent, mac: boolean): boolean {
  return (mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey)
    && !event.altKey && !event.shiftKey && !event.isComposing && !event.getModifierState?.("AltGraph") && event.code === "KeyP";
}
