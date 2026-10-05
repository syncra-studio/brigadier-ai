import { create } from "zustand";
import { persist } from "zustand/middleware";

import { request } from "@/ipc/client";
import type { TerminalOutput } from "@/ipc/generated";
import { useBarActions } from "@/state/barActions";
import { notePaneClose } from "@/state/closedPanes";
import { type AppState, type Selection, useApp } from "@/state/store";

/**
 * The bottom terminal's shells, by place: each conversation has its own (`conv:<id>`), and
 * Home has one set that drafts and Settings show too (`home`). A place remembers its tabs,
 * which one is in front and whether its pane is open. The shells run in the daemon, keyed by
 * their tab's id, and outlive the window; closing a tab, the shell exiting, or its
 * conversation being archived or deleted ends them.
 */

export const HOME_PLACE = "home";

export type TerminalTab = {
  id: string;
  /** The title the shell set, if any. */
  shellTitle: string | null;
  /** Where the shell started, once it has. */
  cwd: string | null;
};

export type TerminalPlace = {
  tabs: TerminalTab[];
  active: string | null;
  open: boolean;
};

const EMPTY: TerminalPlace = { tabs: [], active: null, open: false };

/** What the store held before places: conversations' sessions, keyed by conversation id. */
type SavedSessions = {
  conversations?: Record<
    string,
    { sessions?: { id: string; cwd: string | null }[]; active?: string | null }
  >;
};

export const useTerminalPlaces = create<{ places: Record<string, TerminalPlace> }>()(
  persist(() => ({ places: {} }), {
    // The old store's key, so its tabs carry over and their running shells are found again.
    name: "brigadier.terminalSessions",
    version: 1,
    partialize: ({ places }) => ({ places }),
    migrate: (saved, version) => {
      if (version >= 1) return saved as { places: Record<string, TerminalPlace> };
      const places: Record<string, TerminalPlace> = {};
      for (const [id, value] of Object.entries((saved as SavedSessions).conversations ?? {})) {
        const tabs = (value.sessions ?? []).map((session) => ({
          id: session.id,
          shellTitle: null,
          cwd: session.cwd,
        }));
        if (tabs.length)
          places[`conv:${id}`] = {
            tabs,
            active: tabs.some((tab) => tab.id === value.active) ? value.active! : tabs[0]!.id,
            open: false,
          };
      }
      return { places };
    },
  }),
);

/** The place a selection shows: its conversation's, or Home's. */
export function placeOf(selection: Selection): string {
  return selection.type === "conversation" ? `conv:${selection.id}` : HOME_PLACE;
}

/** The place the window shows now. */
export function currentPlace(): string {
  return placeOf(useApp.getState().selection);
}

/** Whether a terminal can open where the window is: everywhere but an archived thread. */
export function terminalWorksHere(state: AppState): boolean {
  const { selection } = state;
  return (
    selection.type !== "conversation" ||
    state.conversations[selection.id]?.lifecycle !== "archived"
  );
}

export function terminalPlace(place: string): TerminalPlace {
  return useTerminalPlaces.getState().places[place] ?? EMPTY;
}

export function useTerminalPlace(place: string): TerminalPlace {
  return useTerminalPlaces((s) => s.places[place] ?? EMPTY);
}

function update(place: string, change: (current: TerminalPlace) => TerminalPlace): void {
  useTerminalPlaces.setState(({ places }) => {
    const next = change(places[place] ?? EMPTY);
    if (!next.tabs.length && !next.open) {
      const { [place]: _gone, ...rest } = places;
      return { places: rest };
    }
    return { places: { ...places, [place]: next } };
  });
}

/** The daemon's owner for a place's shells: its conversation, or none for Home. */
export function placeConversation(place: string): string | null {
  return place.startsWith("conv:") ? place.slice("conv:".length) : null;
}

// Shells the window has open, by terminal id: which place and tab each belongs to.
const shells = new Map<string, { place: string; tab: string }>();
// Output already shown in a tab when its shell was closed, for bringing it back.
const closed = new Map<string, { tab: TerminalTab; output: string }[]>();
// Output a reopened tab shows before its new shell's, until it closes or is cleared.
const restored = new Map<string, string>();
// Each shown tab's way of reading its output, for keeping it when the tab closes.
const readers = new Map<string, () => string>();

type Listener = (output: TerminalOutput) => void;
const listeners = new Set<Listener>();

/** Called for every terminal's output until the returned function is called. */
export function onTerminalOutput(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** From the bridge: a terminal printed something, or its shell ended (its tab goes). */
export function emitTerminalOutput(output: TerminalOutput): void {
  for (const listener of listeners) listener(output);
  if (output.type !== "exited") return;
  const owner = shells.get(output.terminalId);
  if (!owner) return;
  shells.delete(output.terminalId);
  forgetTab(owner.place, owner.tab);
}

/** The tab's shell is `terminalId` now (a daemon started again gives it a new one). */
export function noteShell(place: string, tab: string, terminalId: string): void {
  for (const [id, owner] of shells) if (owner.tab === tab) shells.delete(id);
  shells.set(terminalId, { place, tab });
}

/** The tab's shell ended while its view watched (perhaps before its owner was known). */
export function noteShellExit(place: string, tab: string): void {
  for (const [id, owner] of shells) if (owner.tab === tab) shells.delete(id);
  forgetTab(place, tab);
}

export function noteShellCwd(place: string, tab: string, cwd: string): void {
  update(place, (current) => ({
    ...current,
    tabs: current.tabs.map((each) => (each.id === tab ? { ...each, cwd } : each)),
  }));
}

export function noteShellTitle(place: string, tab: string, title: string): void {
  const shellTitle = title.trim() || null;
  update(place, (current) => ({
    ...current,
    tabs: current.tabs.map((each) => (each.id === tab ? { ...each, shellTitle } : each)),
  }));
}

/** While a tab shows, how to read what it shows. */
export function noteTabReader(tab: string, read: () => string): () => void {
  readers.set(tab, read);
  return () => {
    if (readers.get(tab) === read) readers.delete(tab);
  };
}

/** Output to show in a reopened tab before its shell's. */
export function restoredOutput(tab: string): string | null {
  return restored.get(tab) ?? null;
}

/** The tab was cleared: what it brought back goes too. */
export function forgetRestoredOutput(tab: string): void {
  restored.delete(tab);
}

export function hasTab(place: string, tab: string): boolean {
  return terminalPlace(place).tabs.some((each) => each.id === tab);
}

/** Each tab's name: the shell's title, else its folder's (numbered when two share one), else
 * "Terminal N". The folder is the project's, even when the shell runs in a worktree of it. */
export function tabNames(tabs: readonly TerminalTab[], projectPath?: string | null): string[] {
  const folder = (tab: TerminalTab) =>
    (projectPath || tab.cwd)?.split(/[\\/]/).filter(Boolean).at(-1) ?? null;
  const counts = new Map<string, number>();
  for (const tab of tabs) {
    const name = folder(tab);
    if (name) counts.set(name, (counts.get(name) ?? 0) + 1);
  }
  const seen = new Map<string, number>();
  return tabs.map((tab, index) => {
    if (tab.shellTitle) return tab.shellTitle;
    const name = folder(tab);
    if (!name) return `Terminal ${index + 1}`;
    const nth = (seen.get(name) ?? 0) + 1;
    seen.set(name, nth);
    return (counts.get(name) ?? 0) > 1 && nth > 1 ? `${name} ${nth}` : name;
  });
}

export function addTab(place: string): string {
  const id = crypto.randomUUID();
  update(place, (current) => ({
    ...current,
    tabs: [...current.tabs, { id, shellTitle: null, cwd: null }],
    active: id,
    open: true,
  }));
  return id;
}

export function selectTab(place: string, tab: string): void {
  update(place, (current) => ({ ...current, active: tab }));
}

/** The place's pane shown or hidden; shown with no tab, it starts one. */
export function setTerminalOpen(place: string, open: boolean): void {
  if (open && !terminalPlace(place).tabs.length) {
    addTab(place);
    return;
  }
  update(place, (current) => ({ ...current, open }));
}

/** Shows or hides the place's pane; hidden under full view, it is shown by leaving full view. */
export function toggleTerminal(place = currentPlace()): void {
  if (place === currentPlace() && !terminalWorksHere(useApp.getState())) return;
  const cover = useBarActions.getState().terminalCover;
  if (cover && place === currentPlace()) {
    cover();
    setTerminalOpen(place, true);
    return;
  }
  setTerminalOpen(place, !terminalPlace(place).open);
}

function forgetTab(place: string, tab: string): void {
  readers.delete(tab);
  restored.delete(tab);
  update(place, (current) => {
    const index = current.tabs.findIndex((each) => each.id === tab);
    if (index < 0) return current;
    const tabs = current.tabs.filter((each) => each.id !== tab);
    return {
      tabs,
      active:
        current.active === tab ? (tabs[Math.max(0, index - 1)]?.id ?? null) : current.active,
      open: tabs.length > 0 && current.open,
    };
  });
}

function endShell(tab: string): void {
  for (const [terminalId, owner] of shells) {
    if (owner.tab !== tab) continue;
    shells.delete(terminalId);
    request({ method: "closeTerminal", terminalId }).catch((error: unknown) => {
      console.error("closing the terminal failed", error);
    });
  }
}

/** Closes a tab and ends its shell; what it showed is kept for ⌘⇧T. */
export function closeTab(place: string, tab: string): void {
  const shown = terminalPlace(place).tabs.find((each) => each.id === tab);
  if (!shown) return;
  const output = readers.get(tab)?.() ?? "";
  closed.set(place, [...(closed.get(place) ?? []), { tab: shown, output }]);
  notePaneClose(placeConversation(place) ?? HOME_PLACE, "terminal");
  endShell(tab);
  forgetTab(place, tab);
}

/** Brings back the place's last closed tab: its output, over a new shell. */
export function undoTabClose(place: string): boolean {
  const stack = closed.get(place);
  const last = stack?.pop();
  if (!last) return false;
  const id = crypto.randomUUID();
  if (last.output) restored.set(id, last.output);
  update(place, (current) => ({
    ...current,
    tabs: [...current.tabs, { ...last.tab, id, shellTitle: null }],
    active: id,
    open: true,
  }));
  return true;
}

/** Forgets a place whose conversation was archived or deleted (the daemon ends its shells). */
function dropPlace(place: string): void {
  for (const tab of terminalPlace(place).tabs) {
    readers.delete(tab.id);
    restored.delete(tab.id);
    for (const [terminalId, owner] of shells) if (owner.tab === tab.id) shells.delete(terminalId);
  }
  closed.delete(place);
  update(place, () => EMPTY);
}

// Places follow the catalog: an archived or missing conversation's go, whether it went now or
// while the app was closed.
useApp.subscribe((state, previous) => {
  if (!state.catalogLoaded) return;
  if (state.conversations === previous.conversations && previous.catalogLoaded) return;
  for (const place of Object.keys(useTerminalPlaces.getState().places)) {
    const id = placeConversation(place);
    if (id === null) continue;
    const conversation = state.conversations[id];
    if (!conversation || conversation.lifecycle === "archived") dropPlace(place);
  }
});
