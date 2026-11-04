import { create } from "zustand";
import { persist } from "zustand/middleware";

export type WorkspacePaneTab = "files" | "search" | "source";
export type WorkspacePaneState = { open: boolean; tabs: Record<string, WorkspacePaneTab> };
export const WORKSPACE_PANE_KEY = "brigadier.workspacePane";
export const WORKSPACE_PANE_WIDTH_KEY = "brigadier.workspacePaneWidth";

/** A session; a project's draft; a chat that has started; a new chat or welcome page. */
export type ToolbarKind = "session" | "draft" | "chat" | "chatDraft";

/**
 * The toolbar's controls, in order: a session or project draft adds its right sidebar's. What has
 * started has the Context card. A New tab button comes first until the tab bar, with its own,
 * shows; before anything has started it stands where Context would, and a session has the tab bar always.
 */
export function toolbarToggles(kind: ToolbarKind, tabbed: boolean): readonly ("context" | "new-tab" | "terminal" | "right-sidebar")[] {
  const newTab = kind !== "session" && !tabbed ? ["new-tab" as const] : [];
  const context = kind === "session" || kind === "chat" ? ["context" as const] : [];
  const sidebar = kind === "session" || kind === "draft" ? ["right-sidebar" as const] : [];
  return [...newTab, ...context, "terminal", ...sidebar];
}

/**
 * Whether the right sidebar and the Workers panel both fit beside the thread's minimum and the
 * left column: when they don't, opening one closes the other.
 */
export function columnsFit(windowWidth: number, leftWidth: number, threadMin: number, workersWidth: number, sidebarWidth: number): boolean {
  return windowWidth >= leftWidth + threadMin + workersWidth + sidebarWidth;
}

export function isWorkspacePaneTab(tab: unknown): tab is WorkspacePaneTab {
  return tab === "files" || tab === "search" || tab === "source";
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

export function openWorkspacePane(id: string, tab: WorkspacePaneTab): void {
  useWorkspacePaneState.setState(({ tabs }) => ({ open: true, tabs: { ...tabs, [id]: tab } }));
}

export function setWorkspacePaneOpen(next: boolean | ((open: boolean) => boolean)): void {
  useWorkspacePaneState.setState(({ open }) => ({ open: typeof next === "function" ? next(open) : next }));
}

/** What a key press asks of the right sidebar: ⌥⌘B shows or hides it, ⇧⌘E shows Files, ⇧⌘F Search, ⌃⇧G Source. */
export function workspacePaneKey(event: KeyboardEvent, mac: boolean): "toggle" | WorkspacePaneTab | null {
  if (event.isComposing || event.getModifierState?.("AltGraph")) return null;
  const command = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
  if (command && event.altKey && !event.shiftKey && event.code === "KeyB") return "toggle";
  if (command && event.shiftKey && !event.altKey && event.code === "KeyE") return "files";
  if (command && event.shiftKey && !event.altKey && event.code === "KeyF") return "search";
  if (event.ctrlKey && event.shiftKey && !event.metaKey && !event.altKey && event.code === "KeyG") return "source";
  return null;
}

export function isFilesSearchKey(event: KeyboardEvent, mac: boolean): boolean {
  return (mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey)
    && !event.altKey && !event.shiftKey && !event.isComposing && !event.getModifierState?.("AltGraph") && event.code === "KeyP";
}
