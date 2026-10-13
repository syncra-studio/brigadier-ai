import { create } from "zustand";
import { persist } from "zustand/middleware";

import { notePaneClose } from "@/state/closedPanes";
import { discardDocument, documentIsSaved } from "@/state/documentDrafts";

/**
 * A session's tabs over its main area: Chat (the conversation, always first and never
 * closed), then the files and the one Review tab it opened. Each session remembers its tabs
 * and the one in front, across switches and restarts.
 */

export const CHAT_TAB = "chat";
export const REVIEW_TAB = "review";
/** The session's mockup (SESSION-FIXES-PLAN.md Q10): one tab, there while it builds and after. */
export const MOCKUP_TAB = "mockup";

/** What the Review tab shows: every change (its scope picker), or one file's diff. */
export type ReviewTarget =
  | { type: "all" }
  | { type: "file"; path: string; staged: boolean };

/**
 * A page, file or browser tab's own chat, beside it in a split: `share` is the chat's part of
 * the tab's width. Shown, folded to a strip at the tab's edge, or hidden behind its button; it
 * goes with its tab. `seenAt` is the chat's last activity when it was last in view, so its
 * button can show what came since.
 */
export type TabChatState = {
  id: string;
  view: "split" | "minimized" | "hidden";
  share: number;
  seenAt?: number;
};

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
  chat?: TabChatState;
};

export type ReviewTabState = {
  kind: "review";
  id: typeof REVIEW_TAB;
  target: ReviewTarget;
  opener: string | null;
};

export type BrowserTabState = {
  kind: "browser"; id: string; opener: string | null; url: string; title: string;
  /** Opened from something an agent gave (a link in its reply, a preview's address). */
  agent?: boolean;
  chat?: TabChatState;
};
export type NewTabState = {
  kind: "newTab"; id: string; opener: string | null;
};
export type TerminalTabState = {
  kind: "terminal"; id: string; opener: string | null; cwd: string | null; title: string;
};
export type SideChatTabState = {
  kind: "sideChat"; id: string; opener: string | null; conversationId: string | null; title: string;
  /** When it was opened (ms), which numbers untitled side chats. Older tabs go by their place. */
  opened?: number;
};
export type PageTabState = {
  kind: "page"; id: string; opener: string | null;
  /** Null until the daemon has made the page: a new one, or an old draft moving in. */
  pageId: string | null;
  /** The page's title as last seen, for the tab before the page loads. */
  title: string;
  /** An old "New file" draft (its key in local storage) whose text becomes this page. */
  draft?: string;
  chat?: TabChatState;
};
/** A tab of the old plain-text "New file", only ever read back from storage to be migrated. */
export type LegacyDocumentTab = {
  kind: "document"; id: string; opener: string | null; name: string;
  savedPath: string | null; relativePath: string | null;
};
export type MockupTabState = { kind: "mockup"; id: typeof MOCKUP_TAB; opener: string | null };
export type SessionTab = FileTab | ReviewTabState | BrowserTabState | TerminalTabState | SideChatTabState | PageTabState | MockupTabState | NewTabState;
export type NewTabKind = "newTab" | "terminal" | "browser" | "sideChat" | "page";

export type SessionTabs = {
  /** The tabs after Chat, in order. */
  tabs: SessionTab[];
  active: string;
  /** The mockup moment (`showMockupTab`) the Mockup tab last appeared for, so a closed one stays closed until the next. */
  mockup?: string;
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
      // Only drafts that became checkout files go now; the rest wait in their page tab until
      // the daemon has saved them as pages.
      try {
        localStorage.setItem("brigadier.sessionTabs", JSON.stringify({ state: { sessions }, version: 1 }));
        const waiting = new Set(Object.values(sessions).flatMap((session) =>
          session.tabs.flatMap((tab) => tab.kind === "page" && tab.draft ? [tab.draft] : [])));
        for (const session of Object.values(previous)) for (const tab of session.tabs as (SessionTab | LegacyDocumentTab)[])
          if (tab.kind === "document" && !waiting.has(tab.id)) discardDocument(tab.id);
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
    const current = sessions[conversationId] ?? EMPTY;
    const changed = change(current);
    // Changes that rebuild the tabs keep the mockup moment unless they set it.
    const next = current.mockup && !("mockup" in changed) ? { ...changed, mockup: current.mockup } : changed;
    if (!next.tabs.length && next.active === CHAT_TAB && !next.mockup) {
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
    const tab = makeTab(id, current.active, kind, cwd);
    return { tabs: insertAfterActive(current, tab), active: id };
  });
  return id;
}

function makeTab(id: string, opener: string | null, kind: NewTabKind, cwd: string | null): SessionTab {
  const base = { id, opener };
  return kind === "newTab" ? { ...base, kind }
    : kind === "browser" ? { ...base, kind, url: "", title: "" }
    : kind === "terminal" ? { ...base, kind, cwd, title: "" }
    : kind === "sideChat" ? { ...base, kind, conversationId: crypto.randomUUID(), title: "", opened: Date.now() }
    : { ...base, kind, pageId: null, title: "" };
}

/** A side chat's title from its first message: the first line with text, on one line, at most 60 characters. */
export function sideChatTitle(message: string): string {
  const line = (message.split("\n").find((each) => each.trim()) ?? message).split(/\s+/).filter(Boolean).join(" ");
  const chars = [...line];
  return chars.length <= 60 ? line : `${chars.slice(0, 59).join("")}…`;
}

/**
 * What a Side chat tab reads. Alone it is just "Side chat"; beside others each shows its own
 * title, or until it has one "Side chat 2", "Side chat 3"… in the order they were opened.
 */
export function sideChatName(tab: SideChatTabState, tabs: readonly SessionTab[]): string {
  const sideChats = tabs.filter((each): each is SideChatTabState => each.kind === "sideChat");
  if (sideChats.length < 2) return "Side chat";
  if (tab.title) return tab.title;
  const order = sideChats.toSorted((a, b) => (a.opened ?? 0) - (b.opened ?? 0)).findIndex((each) => each.id === tab.id);
  return order > 0 ? `Side chat ${order + 1}` : "Side chat";
}

/** A new Browser tab showing `url`, after the tab in front. */
export function openBrowserTab(conversationId: string, url: string, agent = false): string {
  const id = `browser:${crypto.randomUUID()}`;
  update(conversationId, (current) => {
    const tab: BrowserTabState = { kind: "browser", id, opener: current.active, url, title: "", ...(agent ? { agent } : {}) };
    return { tabs: insertAfterActive(current, tab), active: id };
  });
  return id;
}

/**
 * The built-in browser was turned off: every Browser tab goes, and none can be reopened. A
 * session whose tab in front was one shows the tab that opened it.
 */
export function dropBrowserTabs(): void {
  for (const [conversationId, list] of closed) {
    const kept = list.filter((entry) => entry.tab.kind !== "browser");
    if (kept.length) closed.set(conversationId, kept);
    else closed.delete(conversationId);
  }
  for (const [conversationId, current] of Object.entries(useSessionTabs.getState().sessions)) {
    const browsers = current.tabs.filter((tab) => tab.kind === "browser");
    if (!browsers.length) continue;
    update(conversationId, () => {
      let { tabs, active } = current;
      for (const tab of browsers) {
        const left = tabs.filter((entry) => entry.id !== tab.id);
        active = afterClose({ tabs, active }, left, tab);
        tabs = left;
      }
      return { tabs, active };
    });
  }
}

/** Page `pageId` was deleted: every tab showing it closes, and none can be reopened. */
export function dropPageTabs(pageId: string): void {
  const shows = (tab: SessionTab) => tab.kind === "page" && tab.pageId === pageId;
  for (const [conversationId, list] of closed) {
    const kept = list.filter((entry) => !shows(entry.tab));
    if (kept.length) closed.set(conversationId, kept);
    else closed.delete(conversationId);
  }
  for (const [conversationId, current] of Object.entries(useSessionTabs.getState().sessions)) {
    const gone = current.tabs.filter(shows);
    if (!gone.length) continue;
    update(conversationId, () => {
      let { tabs, active } = current;
      for (const tab of gone) {
        const left = tabs.filter((entry) => entry.id !== tab.id);
        active = afterClose({ tabs, active }, left, tab);
        tabs = left;
      }
      return { tabs, active };
    });
  }
}

/** Choosing a tool or entering an address consumes the placeholder, not a tab-close entry. */
export function replaceNewTab(conversationId: string, id: string, kind: Exclude<NewTabKind, "newTab">, cwd: string | null = null, url = ""): void {
  update(conversationId, (current) => ({
    ...current,
    tabs: current.tabs.map((tab) => {
      if (tab.id !== id || tab.kind !== "newTab") return tab;
      const next = makeTab(id, tab.opener, kind, cwd);
      return next.kind === "browser" ? { ...next, url } : next;
    }),
  }));
}

/**
 * Shows page `pageId`: the tab already showing it, else `replace` (a New tab) turned into it,
 * else a new tab after the one in front. Returns the tab's ID.
 */
export function openPageTab(conversationId: string, pageId: string, title: string, replace?: string): string {
  let shown = "";
  update(conversationId, (current) => {
    const existing = current.tabs.find((tab) => tab.kind === "page" && tab.pageId === pageId);
    if (existing) {
      shown = existing.id;
      const tabs = replace ? current.tabs.filter((tab) => !(tab.id === replace && tab.kind === "newTab")) : current.tabs;
      return { ...current, tabs, active: existing.id };
    }
    const placeholder = replace ? current.tabs.find((tab) => tab.id === replace && tab.kind === "newTab") : undefined;
    if (placeholder) {
      shown = placeholder.id;
      return { ...current, active: placeholder.id, tabs: current.tabs.map((tab) =>
        tab.id === placeholder.id ? { kind: "page", id: tab.id, opener: tab.opener, pageId, title } : tab) };
    }
    shown = `page:${crypto.randomUUID()}`;
    const tab: PageTabState = { kind: "page", id: shown, opener: current.active, pageId, title };
    return { tabs: insertAfterActive(current, tab), active: shown };
  });
  return shown;
}

/** File and Review keep their canonical IDs; an existing destination keeps its position. */
function replacePlaceholder(current: SessionTabs, id: string | undefined, tab: SessionTab): SessionTabs | null {
  const placeholder = current.tabs.find((entry) => entry.id === id && entry.kind === "newTab");
  if (!placeholder) return null;
  const exists = current.tabs.some((entry) => entry.id === tab.id);
  const tabs = current.tabs.flatMap((entry) => {
    if (entry.id === placeholder.id) return exists ? [] : [{ ...tab, opener: placeholder.opener }];
    return [entry.id === tab.id ? tab : entry];
  }).map((entry) => entry.opener === placeholder.id ? { ...entry, opener: entry.id === tab.id ? placeholder.opener : tab.id } : entry);
  return { tabs, active: tab.id };
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

/** A file picked from New tab never takes the unrelated preview tab's place. */
export function replaceNewTabWithFile(conversationId: string, id: string, path: string): boolean {
  let replaced = false;
  update(conversationId, (current) => {
    const existing = current.tabs.find((tab) => tab.kind === "file" && tab.path === path);
    const tab: FileTab = existing?.kind === "file" ? { ...existing, preview: false }
      : { kind: "file", id: fileId(path), path, line: null, reveal: 0, preview: false, opener: null };
    const next = replacePlaceholder(current, id, tab);
    replaced = next !== null;
    return next ?? current;
  });
  return replaced;
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
export function openReviewTab(conversationId: string, target: ReviewTarget, replace?: string): void {
  update(conversationId, (current) => {
    const existing = current.tabs.find((tab) => tab.kind === "review");
    const replaced = replacePlaceholder(current, replace, {
      kind: "review", id: REVIEW_TAB, target, opener: existing?.opener ?? current.active,
    });
    if (replaced) return replaced;
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

/**
 * Adds the Mockup tab at the end, without bringing it to the front, when the session's mockup
 * reaches a new `moment` (it starts, a revision, the comparison). Closed, it stays closed until
 * the next one.
 */
export function showMockupTab(conversationId: string, moment: string): void {
  update(conversationId, (current) => current.mockup === moment ? current : {
    ...current,
    mockup: moment,
    tabs: current.tabs.some((tab) => tab.id === MOCKUP_TAB) ? current.tabs
      : [...current.tabs, { kind: "mockup", id: MOCKUP_TAB, opener: CHAT_TAB }],
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

/** A tab without its chat: a tab opened again starts with none, as its chat went when it closed. */
export function withoutChat(tab: SessionTab): SessionTab {
  if (!("chat" in tab)) return tab;
  const { chat: _gone, ...rest } = tab;
  return rest as SessionTab;
}

function remember(conversationId: string, gone: SessionTab[]): void {
  if (!gone.length) return;
  const list = closed.get(conversationId) ?? [];
  for (const tab of gone) {
    list.push({ tab: withoutChat(tab), index: sessionTabs(conversationId).tabs.findIndex((entry) => entry.id === tab.id) });
    if (list.length > 20) { const expired = list.shift()!; for (const listener of discarded) listener(expired.tab); }
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

/** Turns an old document's file name into a page title. */
export function draftTitle(name: string): string {
  return name === "Untitled" ? "" : name.replace(/\.[^.]+$/, "") || name;
}

/**
 * Old "New file" tabs from storage: a clean one saved inside the checkout opens as that file;
 * every other one (unsaved edits, never saved, or saved outside the checkout) becomes a page
 * that takes the draft's text, so no text and no tab is lost.
 */
export function restoreSessionTabs(sessions: Record<string, SessionTabs>, isSaved: (id: string, savedPath: string | null) => boolean = () => false): Record<string, SessionTabs> {
  return Object.fromEntries(Object.entries(sessions).map(([id, session]) => {
    const remapped = new Map<string, string>();
    const tabs = (session.tabs as (SessionTab | LegacyDocumentTab)[]).flatMap((tab): SessionTab[] => {
      if (tab.kind === "terminal") return [{ ...tab, title: "" }];
      if (tab.kind !== "document") return [tab];
      if (tab.savedPath && tab.relativePath && isSaved(tab.id, tab.savedPath)) {
        const file = { kind: "file" as const, id: fileId(tab.relativePath), path: tab.relativePath, preview: false, line: null, reveal: 0, opener: tab.opener };
        remapped.set(tab.id, file.id);
        return [file];
      }
      return [{ kind: "page", id: tab.id, opener: tab.opener, pageId: null, title: draftTitle(tab.name), draft: tab.id }];
    }).filter((tab, index, list) => list.findIndex((entry) => entry.id === tab.id) === index)
      .map((tab) => ({ ...tab, opener: tab.opener ? remapped.get(tab.opener) ?? tab.opener : null }));
    const active = remapped.get(session.active) ?? session.active;
    return [id, { tabs, active: tabs.some((tab) => tab.id === active) ? active : CHAT_TAB, ...(session.mockup ? { mockup: session.mockup } : {}) }];
  }));
}
