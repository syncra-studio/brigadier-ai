import { pageCreate, pageWrite } from "@/ipc/client";
import type { Page, PageContent, PageWriteOutcome } from "@/ipc/generated";
import { discardDocument, documentDraftIds, documentIsSaved, documentText } from "@/state/documentDrafts";
import { changeSessionTab, type PageTabState, useSessionTabs } from "@/state/sessionTabs";

/**
 * A page tab opens before its page exists: ⌥⌘N and New page show at once while the daemon makes
 * the page, and an old "New file" draft becomes a page the first time it is needed. Its draft
 * text stays in local storage until the page holding it is saved.
 */

type MakeIpc = {
  create: () => Promise<Page>;
  write: (id: string, content: PageContent, baseRevision: string) => Promise<PageWriteOutcome>;
};

/** The page made for each tab, so a reopened or remounted tab never makes a second one. */
const making = new Map<string, Promise<string>>();

export function ensurePage(tab: PageTabState, ipc: MakeIpc = { create: pageCreate, write: pageWrite }): Promise<string> {
  if (tab.pageId) return Promise.resolve(tab.pageId);
  const pending = making.get(tab.id);
  if (pending) return pending;
  const made = (async () => {
    const page = await ipc.create();
    if (tab.draft) {
      const outcome = await ipc.write(page.id, { title: tab.title, emoji: null, body: documentText(tab.draft) }, page.revision);
      if (outcome.type !== "saved") throw new Error("Could not move the draft into a page.");
    }
    for (const [conversationId, session] of Object.entries(useSessionTabs.getState().sessions)) {
      if (!session.tabs.some((entry) => entry.id === tab.id)) continue;
      changeSessionTab(conversationId, tab.id, (current) => current.kind === "page"
        ? { kind: "page", id: current.id, opener: current.opener, pageId: page.id, title: current.title } : current);
    }
    if (tab.draft) discardDocument(tab.draft);
    return page.id;
  })();
  making.set(tab.id, made);
  made.catch(() => making.delete(tab.id));
  return made;
}

/** Moves every old draft still waiting in a tab into its page, whether or not its tab is shown. */
export function migrateDrafts(ipc?: MakeIpc): Promise<void> {
  const waiting = Object.values(useSessionTabs.getState().sessions).flatMap((session) =>
    session.tabs.flatMap((tab) => tab.kind === "page" && !tab.pageId && tab.draft ? [tab] : []));
  const referenced = new Set(waiting.map((tab) => tab.draft));
  try {
    for (const draft of documentDraftIds()) {
      if (!referenced.has(draft) && !documentIsSaved(draft)) {
        waiting.push({ kind: "page", id: draft, opener: null, pageId: null, title: "", draft });
      }
    }
  } catch (error) { console.error("Could not read the old document drafts", error); }
  return Promise.allSettled(waiting.map((tab) => ensurePage(tab, ipc))).then((results) => {
    for (const result of results) if (result.status === "rejected") console.error("Could not move a draft into a page", result.reason);
  });
}
