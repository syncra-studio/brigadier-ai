import { create } from "zustand";
import { persist } from "zustand/middleware";

import { notePaneClose } from "@/state/closedPanes";

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

export type SessionTab = FileTab | ReviewTabState;

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
function insertAfterActive(current: SessionTabs, tab: SessionTab): SessionTab[] {
  const at = current.tabs.findIndex((entry) => entry.id === current.active);
  const tabs = [...current.tabs];
  tabs.splice(at + 1, 0, tab);
  return tabs;
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
const closed = new Map<string, SessionTab[]>();

/** Where the window goes when `tab` closes: its opener, else its right, else its left. */
function afterClose(current: SessionTabs, tabs: SessionTab[], tab: SessionTab): string {
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
    list.push(tab);
    notePaneClose(conversationId, "tab");
  }
  closed.set(conversationId, list);
}

export function closeTab(conversationId: string, id: string): void {
  if (id === CHAT_TAB) return;
  const current = sessionTabs(conversationId);
  const tab = current.tabs.find((entry) => entry.id === id);
  if (!tab) return;
  const tabs = current.tabs.filter((entry) => entry.id !== id);
  remember(conversationId, [tab]);
  update(conversationId, () => ({ tabs, active: afterClose(current, tabs, tab) }));
}

/** Closes every tab but Chat and `id`. */
export function closeOtherTabs(conversationId: string, id: string): void {
  const current = sessionTabs(conversationId);
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
export function closeTabsToTheRight(conversationId: string, id: string): void {
  const current = sessionTabs(conversationId);
  const at = id === CHAT_TAB ? -1 : current.tabs.findIndex((tab) => tab.id === id);
  const kept = current.tabs.slice(0, at + 1);
  remember(conversationId, current.tabs.slice(at + 1));
  update(conversationId, () => ({
    tabs: kept,
    active: kept.some((tab) => tab.id === current.active) ? current.active : id,
  }));
}

/** Brings back the last closed tab, if any; true when it did. */
export function reopenTab(conversationId: string): boolean {
  const tab = closed.get(conversationId)?.pop();
  if (!tab) return false;
  update(conversationId, (current) => {
    const others = current.tabs.filter(
      (entry) =>
        entry.id !== tab.id &&
        // A reopened preview takes the place of the one open now.
        !(tab.kind === "file" && tab.preview && entry.kind === "file" && entry.preview),
    );
    return { tabs: [...others, tab], active: tab.id };
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
