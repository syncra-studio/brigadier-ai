import { notePaneClose } from "@/state/closedPanes";
import { create } from "zustand";

import { browserClose, browserNavigate, browserOpen } from "@/ipc/client";
import type { BrowserBounds } from "@/ipc/generated";

/**
 * The Browser tabs' pages as the app knows them, one per conversation: what each shows, heard
 * from its webview in the shell. The page itself lives there, over the tab.
 */

export type BrowserPage = {
  url: string;
  title: string;
  loading: boolean;
  /** The last navigation or download the tab refused, offered to the system browser. */
  blocked: string | null;
};

/** The tabs whose page the shell has made, so it is told where to go, not made again. */
const made = new Set<string>();

export const useBrowsers = create<{ pages: Record<string, BrowserPage> }>(
  () => ({ pages: {} }),
);

function update(id: string, patch: Partial<BrowserPage>): void {
  useBrowsers.setState(({ pages }) => {
    const page = pages[id];
    return page
      ? { pages: { ...pages, [id]: { ...page, ...patch } } }
      : { pages };
  });
}

/** Opens `url` in the conversation's Browser tab, over `bounds`. */
export async function openPage(
  id: string,
  url: string,
  bounds: BrowserBounds,
): Promise<void> {
  const page = useBrowsers.getState().pages[id];
  useBrowsers.setState(({ pages }) => ({
    pages: {
      ...pages,
      [id]: { url, title: page?.title ?? "", loading: true, blocked: null },
    },
  }));
  try {
    if (made.has(id)) {
      await browserNavigate(id, url, bounds);
      return;
    }
    await browserOpen(id, url, bounds, (event) => {
      if (event.type === "load")
        update(id, { url: event.url, loading: event.loading });
      else if (event.type === "title") update(id, { title: event.title });
      else update(id, { blocked: event.url, loading: false });
    });
    made.add(id);
  } catch (error) {
    update(id, { loading: false });
    throw error;
  }
}

export function dismissBlocked(id: string): void {
  update(id, { blocked: null });
}

/** The tab closed, or its conversation did: the page and all it stored go. */
export function closePage(id: string): void {
  if (!useBrowsers.getState().pages[id]) return;
  made.delete(id);
  useBrowsers.setState(({ pages }) => {
    const { [id]: _closed, ...rest } = pages;
    return { pages: rest };
  });
  browserClose(id).catch((error: unknown) =>
    console.error("closing the browser failed", error),
  );
}

export type BrowserTabs = {
  ids: string[];
  active: string;
  restoredUrls?: Record<string, string>;
};
const closedTabs = new Map<string, { url: string | null }[]>();
export const useBrowserTabs = create<{
  conversations: Record<string, BrowserTabs>;
}>(() => ({ conversations: {} }));

export function newBrowserTab(conversationId: string, url?: string): string {
  const id = `page-${crypto.randomUUID()}`;
  useBrowserTabs.setState(({ conversations }) => ({
    conversations: {
      ...conversations,
      [conversationId]: {
        ids: [...(conversations[conversationId]?.ids ?? []), id],
        active: id,
        restoredUrls: { ...conversations[conversationId]?.restoredUrls, ...(url ? { [id]: url } : {}) },
      },
    },
  }));
  return id;
}
export function selectBrowserTab(conversationId: string, id: string): void {
  useBrowserTabs.setState(({ conversations }) => {
    const tabs = conversations[conversationId];
    return tabs
      ? {
          conversations: {
            ...conversations,
            [conversationId]: { ...tabs, active: id },
          },
        }
      : { conversations };
  });
}
export function closeBrowserTab(conversationId: string, id: string): void {
  const url = useBrowsers.getState().pages[id]?.url ?? null;
  closedTabs.set(conversationId, [
    ...(closedTabs.get(conversationId) ?? []),
    { url },
  ]);
  notePaneClose(conversationId, "browser");
  closePage(id);
  useBrowserTabs.setState(({ conversations }) => {
    const tabs = conversations[conversationId];
    if (!tabs) return { conversations };
    const index = tabs.ids.indexOf(id);
    const ids = tabs.ids.filter((tab) => tab !== id);
    const active =
      tabs.active === id
        ? (ids[Math.min(index, ids.length - 1)] ?? "")
        : tabs.active;
    return {
      conversations: { ...conversations, [conversationId]: { ids, active } },
    };
  });
}

export function reopenBrowserTab(conversationId: string): boolean {
  const saved = closedTabs.get(conversationId)?.pop();
  if (!saved) return false;
  const id = newBrowserTab(conversationId);
  if (saved.url)
    useBrowserTabs.setState(({ conversations }) => ({
      conversations: {
        ...conversations,
        [conversationId]: {
          ...conversations[conversationId]!,
          restoredUrls: {
            ...conversations[conversationId]?.restoredUrls,
            [id]: saved.url!,
          },
        },
      },
    }));
  return true;
}
