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
  ready?: boolean;
  favicon?: string | null;
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
      [id]: { url, title: page?.title ?? "", loading: true, blocked: null, ready: made.has(id) },
    },
  }));
  try {
    if (made.has(id)) {
      await browserNavigate(id, url, bounds);
      return;
    }
    await browserOpen(id, url, bounds, (event) => {
      if (event.type === "load")
        update(id, { url: event.url, loading: event.loading,
          ...(event.url !== useBrowsers.getState().pages[id]?.url ? { favicon: null } : {}) });
      else if (event.type === "title") update(id, { title: event.title });
      else if (event.type === "favicon") {
        if (event.url === useBrowsers.getState().pages[id]?.url) update(id, { favicon: event.dataUrl });
      }
      else update(id, { blocked: event.url, loading: false });
    });
    if (!useBrowsers.getState().pages[id]) { await browserClose(id); return; }
    made.add(id);
    update(id, { ready: true });
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
