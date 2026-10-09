import { create } from "zustand";
import { persist } from "zustand/middleware";

import { notePaneClose } from "@/state/closedPanes";
import { discardDocument, documentIsSaved, pruneDocumentDrafts } from "@/state/documentDrafts";

/**
 * A session's tabs over its main area: Chat (the conversation, always first and never
 * closed), then the files and the one Review tab it opened. Each session remembers its tabs
 * and the one in front, across switches and restarts.
 */

export const CHAT_TAB = "chat";
export const REVIEW_TAB = "review";

/** What the Review tab shows: every change (its scope picker), or one file's diff. */
export type ReviewTarget =
  | { type: "all" }
  | { type: "file"; path: string; staged: boolean };

export type FileTab = {
  kind: "file";
  id: string;
  path: string;
  /** The line to bring into view. */
  line: number | null;
  /** Bumped each time the line is asked for again, so the view scrolls to it again. */
  reveal: number;
  /** A preview (italic): the next file opened from the tree takes its place. */
  preview: boolean;
  /** The tab it was opened from, where closing it returns. */
  opener: string | null;
};

export type ReviewTabState = {
  kind: "review";
  id: typeof REVIEW_TAB;
  target: ReviewTarget;
  opener: string | null;
};

export type BrowserTabState = {
  kind: "browser"; id: string; opener: string | null; url: string; title: string;
};
export type TerminalTabState = {
  kind: "terminal"; id: string; opener: string | null; cwd: string | null; title: string;
};
export type SideChatTabState = {
  kind: "sideChat"; id: string; opener: string | null; conversationId: string | null; title: string;
};
export type DocumentTab = {
  kind: "document"; id: string; opener: string | null; name: string;
  /** Display only. Rust owns the authority to save to this path. */
  savedPath: string | null; relativePath: string | null;
};
export type SessionTab = FileTab | ReviewTabState | BrowserTabState | TerminalTabState | SideChatTabState | DocumentTab;
export type NewTabKind = "terminal" | "browser" | "sideChat" | "document";

export type SessionTabs = {
  /** The tabs after Chat, in order. */
  tabs: SessionTab[];
  active: string;
};

const EMPTY: SessionTabs = { tabs: [], active: CHAT_TAB };

export const useSessionTabs = create<{ sessions: Record<string, SessionTabs> }>()(
  persist(() => ({ sessions: {} }), {
    name: "brigadier.sessionTabs",
    version: 1,
    partialize: ({ sessions }) => ({ sessions }),
    merge: (saved) => {
      const previous = (saved as { sessions?: Record<string, SessionTabs> })?.sessions ?? {};
      const sessions = restoreSessionTabs(previous, documentIsSaved);
      // Store the converted tabs before dropping their drafts, including if the user quits
      // without touching a tab. A failed storage write must leave the draft recoverable.
      try {
        localStorage.setItem("brigadier.sessionTabs", JSON.stringify({ state: { sessions }, version: 1 }));
        for (const session of Object.values(previous)) for (const tab of session.tabs)
          if (tab.kind === "document" && tab.savedPath && documentIsSaved(tab.id)) discardDocument(tab.id);
        pruneDocumentDrafts(new Set(Object.values(sessions).flatMap((session) =>
          session.tabs.filter((tab) => tab.kind === "document").map((tab) => tab.id))));
      } catch { /* Retain draft keys when storage is unavailable. */ }
      return { sessions };
    },
  }),
);

export function sessionTabs(conversationId: string): SessionTabs {
  return useSessionTabs.getState().sessions[conversationId] ?? EMPTY;
}

export function useSessionTabsOf(conversationId: string | null): SessionTabs {
  return useSessionTabs((s) => (conversationId ? (s.sessions[conversationId] ?? EMPTY) : EMPTY));
}

function update(conversationId: string, change: (current: SessionTabs) => SessionTabs): void {
  useSessionTabs.setState(({ sessions }) => {
    const next = change(sessions[conversationId] ?? EMPTY);
    if (!next.tabs.length && next.active === CHAT_TAB) {
      const { [conversationId]: _gone, ...rest } = sessions;
      return { sessions: rest };
    }
    return { sessions: { ...sessions, [conversationId]: next } };
  });
}

/** A new tab goes right after the one in front. */
export function insertAfterActive(current: SessionTabs, tab: SessionTab): SessionTab[] {
  const at = current.tabs.findIndex((entry) => entry.id === current.active);
  const tabs = [...current.tabs];
  tabs.splice(at + 1, 0, tab);
  return tabs;
}

export function newSessionTab(conversationId: string, kind: NewTabKind, cwd: string | null = null): string {
  const id = `${kind}:${crypto.randomUUID()}`;
  update(conversationId, (current) => {
    const base = { id, opener: current.active };
    const tab: SessionTab = kind === "browser" ? { ...base, kind, url: "", title: "" }
      : kind === "terminal" ? { ...base, kind, cwd, title: "" }
      : kind === "sideChat" ? { ...base, kind, conversationId: crypto.randomUUID(), title: "" }
      : { ...base, kind, name: "Untitled", savedPath: null, relativePath: null };
    return { tabs: insertAfterActive(current, tab), active: id };
  });
  return id;
}

export function changeSessionTab(conversationId: string, id: string, change: (tab: SessionTab) => SessionTab): void {
  update(conversationId, (current) => ({
    ...current, tabs: current.tabs.map((tab) => tab.id === id ? change(tab) : tab),
  }));
}

function fileId(path: string): string {
  return `file:${path}`;
}

/**
 * Opens `path` in a tab and brings it to the front: a tab that already shows it, else (for a
 * `preview`) the preview tab in its place, else a new tab. Opening it for good (not
 * `preview`) keeps a preview tab of it open for good too.
 */
export function openFileTab(
  conversationId: string,
  path: string,
  { line = null, preview = false }: { line?: number | null; preview?: boolean } = {},
): void {
  update(conversationId, (current) => {
    const id = fileId(path);
    const existing = current.tabs.find((tab) => tab.id === id);
    if (existing && existing.kind === "file") {
      const tabs = current.tabs.map((tab) =>
        tab.id === id && tab.kind === "file"
          ? {
              ...tab,
              preview: tab.preview && preview,
              line: line ?? tab.line,
              reveal: line === null ? tab.reveal : tab.reveal + 1,
            }
          : tab,
      );
      return { tabs, active: id };
    }
    const opener = current.active;
    const tab: FileTab = { kind: "file", id, path, line, reveal: 0, preview, opener };
    const previewAt = preview
      ? current.tabs.findIndex((entry) => entry.kind === "file" && entry.preview)
      : -1;
    if (previewAt >= 0) {
      const replaced = current.tabs[previewAt]!;
      const tabs = [...current.tabs];
      // It keeps the place and the opener of the preview it replaces.
      tabs[previewAt] = { ...tab, opener: replaced.opener === id ? null : replaced.opener };
      return { tabs, active: id };
    }
    return { tabs: insertAfterActive(current, tab), active: id };
  });
}

/** Keeps a preview tab open for good. */
export function keepTabOpen(conversationId: string, id: string): void {
  update(conversationId, (current) => ({
    ...current,
    tabs: current.tabs.map((tab) =>
      tab.id === id && tab.kind === "file" ? { ...tab, preview: false } : tab,
    ),
  }));
}

/** Opens the Review tab on `target`, in place of what it showed. */
export function openReviewTab(conversationId: string, target: ReviewTarget): void {
  update(conversationId, (current) => {
    if (current.tabs.some((tab) => tab.id === REVIEW_TAB)) {
      return {
        tabs: current.tabs.map((tab) => (tab.kind === "review" ? { ...tab, target } : tab)),
        active: REVIEW_TAB,
      };
    }
    const tab: ReviewTabState = {
      kind: "review",
      id: REVIEW_TAB,
      target,
      opener: current.active,
    };
    return { tabs: insertAfterActive(current, tab), active: REVIEW_TAB };
  });
}

export function selectTab(conversationId: string, id: string): void {
  update(conversationId, (current) =>
    id === CHAT_TAB || current.tabs.some((tab) => tab.id === id)
      ? { ...current, active: id }
      : current,
  );
}

/** The tabs closed in each session, newest last, for ⌘⇧T. */
const closed = new Map<string, { tab: SessionTab; index: number }[]>();
const discarded = new Set<(tab: SessionTab) => void>();
export function onDiscardTab(listener: (tab: SessionTab) => void): () => void { discarded.add(listener); return () => { discarded.delete(listener); }; }
export function discardTab(tab: SessionTab): void { for (const listener of discarded) listener(tab); }

/** Where the window goes when `tab` closes: its opener, else its right, else its left. */
export function afterClose(current: SessionTabs, tabs: SessionTab[], tab: SessionTab): string {
  if (current.active !== tab.id) return current.active;
  if (tab.opener && (tab.opener === CHAT_TAB || tabs.some((entry) => entry.id === tab.opener)))
    return tab.opener;
  const at = current.tabs.findIndex((entry) => entry.id === tab.id);
  return tabs[at]?.id ?? tabs[at - 1]?.id ?? CHAT_TAB;
}

function remember(conversationId: string, gone: SessionTab[]): void {
  if (!gone.length) return;
  const list = closed.get(conversationId) ?? [];
  for (const tab of gone) {
    list.push({ tab, index: sessionTabs(conversationId).tabs.findIndex((entry) => entry.id === tab.id) });
    if (list.length > 20) { const expired = list.shift()!; for (const listener of discarded) listener(expired.tab); }
    notePaneClose(conversationId, "tab");
  }
  closed.set(conversationId, list);
}

export const useTabCloseAsk = create<{ confirm: (() => void) | null }>(() => ({ confirm: null }));

function askToClose(tabs: SessionTab[], confirm: () => void): boolean {
  if (!tabs.some((tab) => tab.kind === "document" && !documentIsSaved(tab.id))) return false;
  useTabCloseAsk.setState({ confirm });
  return true;
}

export function closeTab(conversationId: string, id: string, confirmed = false): void {
  if (id === CHAT_TAB) return;
  const current = sessionTabs(conversationId);
  const tab = current.tabs.find((entry) => entry.id === id);
  if (!tab) return;
  if (!confirmed && askToClose([tab], () => closeTab(conversationId, id, true))) return;
  const tabs = current.tabs.filter((entry) => entry.id !== id);
  remember(conversationId, [tab]);
  update(conversationId, () => ({ tabs, active: afterClose(current, tabs, tab) }));
}

/** Closes every tab but Chat and `id`. */
export function closeOtherTabs(conversationId: string, id: string, confirmed = false): void {
  const current = sessionTabs(conversationId);
  if (!confirmed && askToClose(current.tabs.filter((tab) => tab.id !== id), () => closeOtherTabs(conversationId, id, true))) return;
  remember(
    conversationId,
    current.tabs.filter((tab) => tab.id !== id),
  );
  update(conversationId, () => ({
    tabs: current.tabs.filter((tab) => tab.id === id),
    active: id,
  }));
}

/** Closes the tabs to the right of `id` (every tab, for Chat). */
export function closeTabsToTheRight(conversationId: string, id: string, confirmed = false): void {
  const current = sessionTabs(conversationId);
  const at = id === CHAT_TAB ? -1 : current.tabs.findIndex((tab) => tab.id === id);
  if (!confirmed && askToClose(current.tabs.slice(at + 1), () => closeTabsToTheRight(conversationId, id, true))) return;
  const kept = current.tabs.slice(0, at + 1);
  remember(conversationId, current.tabs.slice(at + 1));
  update(conversationId, () => ({
    tabs: kept,
    active: kept.some((tab) => tab.id === current.active) ? current.active : id,
  }));
}

/** Brings back the last closed tab, if any; true when it did. */
export function reopenTab(conversationId: string, allowTools = true): boolean {
  const saved = closed.get(conversationId)?.pop();
  if (!saved) return false;
  const { tab, index } = saved;
  if (!allowTools && (tab.kind === "terminal" || tab.kind === "sideChat")) {
    for (const listener of discarded) listener(tab);
    return false;
  }
  update(conversationId, (current) => {
    const others = current.tabs.filter(
      (entry) =>
        entry.id !== tab.id &&
        // A reopened preview takes the place of the one open now.
        !(tab.kind === "file" && tab.preview && entry.kind === "file" && entry.preview),
    );
    others.splice(Math.min(index, others.length), 0, tab);
    return { tabs: others, active: tab.id };
  });
  return true;
}

/** Moves `id` to `to` among the tabs after Chat. */
export function moveTab(conversationId: string, id: string, to: number): void {
  update(conversationId, (current) => {
    const tab = current.tabs.find((entry) => entry.id === id);
    if (!tab) return current;
    const tabs = current.tabs.filter((entry) => entry.id !== id);
    tabs.splice(Math.max(0, Math.min(to, tabs.length)), 0, tab);
    return { ...current, tabs };
  });
}

/** Every tab's id in order, Chat first. */
export function tabOrder(current: SessionTabs): string[] {
  return [CHAT_TAB, ...current.tabs.map((tab) => tab.id)];
}

/** The next (or, with `step` -1, the previous) tab, round the ends. */
export function stepTab(conversationId: string, step: 1 | -1): void {
  const current = sessionTabs(conversationId);
  const order = tabOrder(current);
  const at = order.indexOf(current.active);
  selectTab(conversationId, order[(at + step + order.length) % order.length]!);
}

/** ⌘1 is Chat, ⌘2 to ⌘9 the tabs after it (⌘9 the last, as browsers do). */
export function selectTabNumber(conversationId: string, number: number): void {
  const order = tabOrder(sessionTabs(conversationId));
  const id = number === 9 ? order.at(-1) : order[number - 1];
  if (id) selectTab(conversationId, id);
}

/** Only clean saved documents return as checkout files. Dirty drafts need a new native save grant. */
export function restoreSessionTabs(sessions: Record<string, SessionTabs>, isSaved: (id: string) => boolean = () => false): Record<string, SessionTabs> {
  return Object.fromEntries(Object.entries(sessions).map(([id, session]) => {
    const remapped = new Map<string, string>();
    const tabs = session.tabs.flatMap((tab): SessionTab[] => {
      if (tab.kind === "terminal") return [{ ...tab, title: "" }];
      if (tab.kind !== "document" || !tab.savedPath) return [tab];
      if (!isSaved(tab.id)) return [{ ...tab, savedPath: null, relativePath: null }];
      if (!tab.relativePath) return [];
      const file = { kind: "file" as const, id: fileId(tab.relativePath), path: tab.relativePath, preview: false, line: null, reveal: 0, opener: tab.opener };
      remapped.set(tab.id, file.id);
      return [file];
    }).filter((tab, index, list) => list.findIndex((entry) => entry.id === tab.id) === index)
      .map((tab) => ({ ...tab, opener: tab.opener ? remapped.get(tab.opener) ?? tab.opener : null }));
    const active = remapped.get(session.active) ?? session.active;
    return [id, { tabs, active: tabs.some((tab) => tab.id === active) ? active : CHAT_TAB }];
  }));
}
