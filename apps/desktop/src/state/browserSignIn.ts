import { create } from "zustand";

import { browserAgentSignInStop } from "@/ipc/client";
import { driverOf, isDriven, pausedFor, useAgentPause } from "@/state/agentPause";
import { selectTab } from "@/state/sessionTabs";

/**
 * Pages only the user goes through (electron/main/browserSignIn.ts): an agent driving a tab stops at a sign-in page or
 * a captcha, the tab comes to the front of its session, and the agent's next call waits for Continue. Nothing resumes
 * by itself. Agents never sign in or answer a challenge: Google's rules forbid automation there, and their password
 * rules wouldn't let them anyway.
 */
export type SignInStop = { kind: "signIn" | "captcha" | "rejected"; site: string; key: string; window?: boolean };
export type SignInSeen = { look?: boolean; stop?: SignInStop; rejected?: boolean };

/** The Browser tabs showing Google's refusal, in their page or a popup of theirs, whose card the user hasn't dismissed. */
export const useSignInRejected = create<Record<string, true>>(() => ({}));

export function dismissSignInRejected(tab: string): void {
  useSignInRejected.setState(({ [tab]: _gone, ...rest }) => rest, true);
}

/** The stop each tab last had: the user's Continue answers it, until its page stops being one. */
const answered = new Map<string, string>();

/** Stops the agent driving `tab` for `stop`, unless the user has already answered it. Whether it stopped now. */
export function stopAgent(tab: string, stop: SignInStop): boolean {
  if (!isDriven(tab) || answered.get(tab) === stop.key) return false;
  answered.set(tab, stop.key);
  pausedFor(tab, stop);
  // The tab's the user's now, so they need to see it. Only within its session: no window moves and no focus goes.
  const owner = driverOf(tab);
  if (owner) selectTab(owner.conversationId, tab);
  return true;
}

/** The tab's page needs nobody, so a later sign-in on it is a new one. */
export const signInCleared = (tab: string) => answered.delete(tab);

export function signInSeen(tab: string, { look, stop, rejected }: SignInSeen): void {
  if (rejected === true) useSignInRejected.setState({ [tab]: true });
  if (rejected === false) dismissSignInRejected(tab);
  if (stop) stopAgent(tab, stop);
  if (!look || !isDriven(tab)) return;
  browserAgentSignInStop(tab).then((found) => {
    if (found) stopAgent(tab, found);
    else signInCleared(tab);
  }).catch((error: unknown) => console.error("looking for a sign-in page failed", error));
}

/** What the agent hears in place of a page it stopped at: not the page, since the tab is the user's now. */
export function stoppedWords(stop: SignInStop, tab: string): { agent: string } {
  const after = `The user has this tab now and nothing more was done here. Tell them what you need. Your next call on ${tab} waits until they press Continue.`;
  if (stop.kind === "rejected") return { agent: `Stopped: Google refused this sign-in for this browser (${stop.site}). Agents can't help with that: only the user can decide what to do next. ${after}` };
  if (stop.kind === "captcha") return { agent: `Stopped: ${stop.site} shows a captcha. Agents never answer these, and only the user can. ${after}` };
  if (stop.window) return { agent: `Stopped: this page opened a separate sign-in window at ${stop.site}. You can't see or use that window, and agents don't sign in. ${after}` };
  return { agent: `Stopped: ${stop.site} is asking for a sign-in. Agents don't sign in: only the user does. ${after}` };
}

export const askedOf = (tab: string): SignInStop | undefined => useAgentPause.getState().asked[tab];
