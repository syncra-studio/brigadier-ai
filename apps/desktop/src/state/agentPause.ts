import { create } from "zustand";

import { request } from "@/ipc/client";
import type { SignInStop } from "@/state/browserSignIn";
import { type BrowserDriver, sessionTabs, sessionWith } from "@/state/sessionTabs";

/**
 * The user's takeover of an agent's Browser tab (Q74): the user's own input in the tab, its
 * page or its toolbar, pauses the agent working it; Continue hands it back. Kept here, apart
 * from the page, so the page loading again can't undo it. The daemon holds the agent's calls
 * on a paused tab.
 */
export const useAgentPause = create<{ paused: Record<string, string>; devtools: Record<string, true>; asked: Record<string, SignInStop> }>(() => ({ paused: {}, devtools: {}, asked: {} }));

/** The agent working Browser tab `id`, with its session; null for the user's own tabs. */
export function driverOf(id: string): { conversationId: string; driver: BrowserDriver } | null {
  const conversationId = sessionWith(id);
  if (!conversationId) return null;
  const tab = sessionTabs(conversationId).tabs.find((each) => each.id === id);
  return tab?.kind === "browser" && tab.driver ? { conversationId, driver: tab.driver } : null;
}

/**
 * Whether an agent is driving Browser tab `id`: it has the tab and the user hasn't taken it over. For autofill and save
 * offers (section 6): a driven tab gets neither.
 */
export const isDriven = (id: string): boolean => driverOf(id) !== null && !(id in useAgentPause.getState().paused);

export const isPaused = (id: string): boolean => id in useAgentPause.getState().paused;

/** DevTools is open on the tab: a call gets `devtools_open` at once, and reads of its activity go on. */
export const devToolsOpen = (id: string): boolean => id in useAgentPause.getState().devtools;

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

/** Tab `id`'s page is for the user alone (state/browserSignIn.ts): its agent pauses as if they had taken it over. */
export function pausedFor(id: string, stop: SignInStop): void {
  if (isPaused(id)) return;
  const owner = driverOf(id);
  if (!owner) return;
  useAgentPause.setState(({ paused, asked }) => ({ paused: { ...paused, [id]: owner.conversationId }, asked: { ...asked, [id]: stop } }));
  tell(owner.conversationId, id, true);
}

/**
 * DevTools opened or closed on tab `id`. Opening it on an agent's tab counts as taking it over. Closing it resumes
 * nothing: the agent waits for Continue again. The daemon holds a paused tab's calls, so it hears of the pause only
 * while DevTools is shut: while open, the calls come here and are answered.
 */
export function devToolsChanged(id: string, open: boolean): void {
  if (open === devToolsOpen(id)) return;
  const owner = driverOf(id);
  if (open) {
    const was = isPaused(id);
    useAgentPause.setState(({ paused, devtools }) => ({
      devtools: { ...devtools, [id]: true },
      paused: owner ? { ...paused, [id]: owner.conversationId } : paused,
    }));
    if (owner && was) tell(owner.conversationId, id, false);
    return;
  }
  useAgentPause.setState(({ devtools }) => {
    const { [id]: _gone, ...rest } = devtools;
    return { devtools: rest };
  });
  if (owner && isPaused(id)) tell(owner.conversationId, id, true);
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
  useAgentPause.setState(({ paused, devtools, asked }) => {
    const { [id]: _gone, ...rest } = paused;
    const { [id]: _open, ...shut } = devtools;
    const { [id]: _stop, ...others } = asked;
    return { paused: rest, devtools: shut, asked: others };
  });
  tell(conversationId, id, false);
}
