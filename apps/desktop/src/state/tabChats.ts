import { request } from "@/ipc/client";
import type { TabTarget } from "@/ipc/generated";
import {
  type BrowserTabState,
  changeSessionTab,
  type FileTab,
  type PageTabState,
  type SessionTab,
  type SessionTabs,
  type TabChatState,
  useSessionTabs,
} from "@/state/sessionTabs";

/**
 * A page, file or browser tab's own chat (decisions Q9–Q12): it opens in a split beside the
 * tab, belongs to that one tab and goes, with all of it, when the tab closes. The daemon keeps
 * it while the tab is there, across restarts.
 */

/** The tabs that can have a chat. */
export type ChatTab = PageTabState | FileTab | BrowserTabState;

/** The chat's part of the tab's width at first, and how far the divider may go either way. */
export const DEFAULT_SHARE = 0.5;
export const MIN_SHARE = 0.25;
export const MAX_SHARE = 0.75;

export function canChat(tab: SessionTab): tab is ChatTab {
  return tab.kind === "page" || tab.kind === "file" || tab.kind === "browser";
}

/** What the tab's chat is about; none for a page not made yet. */
export function chatTarget(tab: ChatTab): TabTarget | null {
  if (tab.kind === "page") return tab.pageId ? { type: "page", pageId: tab.pageId } : null;
  if (tab.kind === "file") return { type: "file", path: tab.path };
  return { type: "browser" };
}

function changeChat(conversationId: string, tabId: string, change: (chat: TabChatState | undefined) => TabChatState | undefined): void {
  changeSessionTab(conversationId, tabId, (tab) => {
    if (!canChat(tab)) return tab;
    const chat = change(tab.chat);
    if (chat === tab.chat) return tab;
    if (chat) return { ...tab, chat };
    const { chat: _gone, ...rest } = tab;
    return rest as ChatTab;
  });
}

/** Opens the tab's chat beside it: the one it has, as wide as it was, or a new one. */
export function showTabChat(conversationId: string, tabId: string): void {
  changeChat(conversationId, tabId, (chat) =>
    chat ? { ...chat, view: "split" } : { id: crypto.randomUUID(), view: "split", share: DEFAULT_SHARE });
}

/** Folds the chat to a strip at the tab's edge, or hides it behind its button. */
export function setTabChatView(conversationId: string, tabId: string, view: "minimized" | "hidden", seenAt?: number): void {
  changeChat(conversationId, tabId, (chat) => chat && { ...chat, view, ...(seenAt === undefined ? {} : { seenAt }) });
}

export function setTabChatShare(conversationId: string, tabId: string, share: number): void {
  const within = Math.min(MAX_SHARE, Math.max(MIN_SHARE, share));
  changeChat(conversationId, tabId, (chat) => chat && chat.share !== within ? { ...chat, share: within } : chat);
}

/** Starts the tab's chat over: a new, empty one in its place, as wide; the old one goes. */
export function newTabChat(conversationId: string, tabId: string): void {
  changeChat(conversationId, tabId, (chat) => ({ id: crypto.randomUUID(), view: "split", share: chat?.share ?? DEFAULT_SHARE }));
}

/** Every tab chat the tabs have, by id. */
export function tabChatIds(sessions: Record<string, SessionTabs>): Set<string> {
  return new Set(Object.values(sessions).flatMap((session) =>
    session.tabs.flatMap((tab) => canChat(tab) && tab.chat ? [tab.chat.id] : [])));
}

/** Deletes a tab chat for good: the daemon stops it if it works and removes all of it. */
function wipe(id: string): void {
  void request({ method: "delete", ids: [id] }).catch(console.error);
}

/** Tab chats that left the tabs (a tab closed, a page deleted, New chat) go at once. */
export function followTabChats(state: { sessions: Record<string, SessionTabs> }, before: { sessions: Record<string, SessionTabs> }): void {
  if (state.sessions === before.sessions) return;
  const kept = tabChatIds(state.sessions);
  for (const id of tabChatIds(before.sessions)) if (!kept.has(id)) wipe(id);
}

/**
 * Tells the daemon which tab chats the tabs have, after every connect: the rest go (their tab
 * closed while Brigadier or the app was down), and until it hears this none runs.
 */
export async function keepTabChats(): Promise<void> {
  await request({ method: "keepTabChats", chatIds: [...tabChatIds(useSessionTabs.getState().sessions)] });
}

/** What runs before each message a tab chat sends: a browser tab sends what its page shows. */
const preparers = new Map<string, () => Promise<void>>();
export function prepareBeforeSend(chatId: string, prepare: () => Promise<void>): () => void {
  preparers.set(chatId, prepare);
  return () => { if (preparers.get(chatId) === prepare) preparers.delete(chatId); };
}
export async function prepareTabChatSend(chatId: string): Promise<void> {
  // A page that can't be read still lets the message go: the chat says what it lacks.
  await preparers.get(chatId)?.().catch(console.error);
}
