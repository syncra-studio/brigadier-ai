import { create } from "zustand";

import { request } from "@/ipc/client";
import { type BrowserDriver, sessionTabs, sessionWith } from "@/state/sessionTabs";

/**
 * The user's takeover of an agent's Browser tab (Q74): the user's own input in the tab, its
 * page or its toolbar, pauses the agent working it; Continue hands it back. Kept here, apart
 * from the page, so the page loading again can't undo it. The daemon holds the agent's calls
 * on a paused tab.
 */
export const useAgentPause = create<{ paused: Record<string, string> }>(() => ({ paused: {} }));

/** The agent working Browser tab `id`, with its session; null for the user's own tabs. */
export function driverOf(id: string): { conversationId: string; driver: BrowserDriver } | null {
  const conversationId = sessionWith(id);
  if (!conversationId) return null;
  const tab = sessionTabs(conversationId).tabs.find((each) => each.id === id);
  return tab?.kind === "browser" && tab.driver ? { conversationId, driver: tab.driver } : null;
}

export const isPaused = (id: string): boolean => id in useAgentPause.getState().paused;

function tell(conversationId: string, tab: string, paused: boolean): void {
  request({ method: "browserPaused", conversationId, tab, paused }).catch((error: unknown) =>
    console.error("telling the daemon about a paused tab failed", error),
  );
}

/** The user clicked, typed or scrolled in tab `id`, or used its toolbar: an agent's tab pauses. */
export function userUsed(id: string): void {
  if (isPaused(id)) return;
  const owner = driverOf(id);
  if (!owner) return;
  useAgentPause.setState(({ paused }) => ({ paused: { ...paused, [id]: owner.conversationId } }));
  tell(owner.conversationId, id, true);
}

/** The Browser tabs whose page shows now. */
const showing = new Set<string>();

/** Tab `id`'s page shows over its tab, or stops showing (its tab went behind, closed). */
export function pageShows(id: string, shows: boolean): void {
  if (shows) showing.add(id);
  else showing.delete(id);
}

/**
 * The page of tab `id` heard the user's click, key or scroll. Only a page that shows can be the
 * user's: one behind its tab (an agent's tab the user never opened) hears keys and scrolls
 * meant for the app, which would pause its agent (#416).
 */
export function pageUsed(id: string): void {
  if (showing.has(id)) userUsed(id);
}

/** Continue: the agent goes on in tab `id`; also when the tab closes. */
export function handBack(id: string): void {
  const conversationId = useAgentPause.getState().paused[id];
  if (conversationId === undefined) return;
  useAgentPause.setState(({ paused }) => {
    const { [id]: _gone, ...rest } = paused;
    return { paused: rest };
  });
  tell(conversationId, id, false);
}
