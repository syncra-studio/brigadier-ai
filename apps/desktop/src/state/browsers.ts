import { create } from "zustand";
import { stepZoom } from "@/app/conversation/browserKeys";

import { browserClose, browserNavigate, browserOpen, browserZoom, revealPath } from "@/ipc/client";
import type { BrowserBounds } from "@/ipc/generated";
import { closeFind, findState, forgetFind } from "@/state/browserFind";
import { closeTabAnywhere, openPopupTab } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

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
  /** The page zoom (1 is 100%) the shell last said; it stays from page to page in the tab. */
  zoom?: number;
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

/**
 * Whether a favicon sent from `url` belongs to the tab's `page`: the same document, also after a
 * hash change, or a same-origin `pushState` once the page has loaded. While a new page loads, the
 * previous one's late favicons are refused; another origin's always are.
 */
export function faviconFits(page: Pick<BrowserPage, "url" | "loading"> | undefined, url: string): boolean {
  if (!page) return false;
  if (url === page.url) return true;
  try {
    const sent = new URL(url);
    const shown = new URL(page.url);
    if (sent.origin !== shown.origin) return false;
    sent.hash = shown.hash = "";
    return sent.href === shown.href || !page.loading;
  } catch {
    return false;
  }
}

/** Opens `url` in the Browser tab `id`, over `bounds`; `agent`: an agent gave the tab its page. */
export async function openPage(
  id: string,
  url: string,
  bounds: BrowserBounds,
  agent = false,
): Promise<void> {
  const page = useBrowsers.getState().pages[id];
  useBrowsers.setState(({ pages }) => ({
    pages: {
      ...pages,
      [id]: {
        url,
        title: page?.title ?? "",
        loading: true,
        blocked: null,
        ready: made.has(id),
        ...(page?.zoom === undefined ? {} : { zoom: page.zoom }),
      },
    },
  }));
  try {
    if (made.has(id)) {
      await browserNavigate(id, url, bounds);
      return;
    }
    await browserOpen(id, url, bounds, agent, (event) => {
      if (event.type === "load")
        update(id, { url: event.url, loading: event.loading,
          ...(event.url !== useBrowsers.getState().pages[id]?.url ? { favicon: null } : {}) });
      else if (event.type === "title") update(id, { title: event.title });
      else if (event.type === "favicon") {
        if (faviconFits(useBrowsers.getState().pages[id], event.url)) update(id, { favicon: event.dataUrl });
      }
      else if (event.type === "downloaded") downloaded(event.path, event.ok);
      else if (event.type === "escape") {
        if (findState(id).open) closeFind(id);
      }
      else if (event.type === "zoomIn") {
        const zoom = useBrowsers.getState().pages[id]?.zoom ?? 1;
        zoomPage(id, stepZoom(zoom, 1)).catch(() => {});
      }
      else if (event.type === "popup") {
        // Its page waits for a tab; with the opener's tab gone it has none.
        if (!openPopupTab(id, event.id, event.url)) browserClose(event.id).catch(() => {});
      }
      else if (event.type === "closed") closeTabAnywhere(id);
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

/** A download finished or failed: a toast says so, and finds the file. */
function downloaded(path: string, ok: boolean): void {
  const name = path.split(/[\\/]/).pop() || path;
  if (!ok) {
    toast(`Couldn't download ${name}`, { tone: "error" });
    return;
  }
  const mac = useApp.getState().info?.platform === "macos";
  toast(`Downloaded ${name}`, {
    actions: [{ label: mac ? "Reveal in Finder" : "Show in folder", run: () => void revealPath(path) }],
  });
}

/** Zooms the tab's page to `zoom` (1 is 100%), keeping what the shell says it came to. */
export async function zoomPage(id: string, zoom: number): Promise<void> {
  update(id, { zoom: await browserZoom(id, zoom) });
}

export function dismissBlocked(id: string): void {
  update(id, { blocked: null });
}

/** The tab closed, or its conversation did: the page and all it stored go. */
export function closePage(id: string): void {
  forgetFind(id);
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
