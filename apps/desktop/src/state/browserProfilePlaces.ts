import { invoke } from "@/ipc/native";
import { create } from "zustand";

import { sessionTabs, sessionWith, useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

/**
 * The built-in browser's profiles: the list, the default, and which profile each session and tab
 * is on, kept by the shell in one file (electron/main/profileStore.ts). A session's new tabs open
 * on its profile, and a tab stays on the profile it opened on until the user moves it. New sessions
 * start on the default: a session has an entry only once it opens a page or the user picks for it.
 */

/** A profile: its own partition, a colour (`--glyph-1` to `--glyph-8`) and a name whose first letter marks it. */
export type BrowserProfile = {
  id: string;
  name: string;
  hue: number;
  /** On its way out: saved first, so a crash finishes the job at the next start. Nothing new opens on it. */
  deleting?: boolean;
};

/** The profile there is before any is made. Pages from before profiles are in its folder. */
export const STARTER: BrowserProfile = { id: "default", name: "Personal", hue: 6 };

/** The colours in the order new profiles take them. */
export const HUES = [6, 4, 1, 5, 2, 3, 7, 8] as const;

export type ProfilePlaces = {
  profiles: BrowserProfile[];
  defaultId: string;
  sessions: Record<string, string>;
  tabs: Record<string, string>;
};

export const useProfilePlaces = create<ProfilePlaces>(() => ({
  profiles: [STARTER],
  defaultId: STARTER.id,
  sessions: {},
  tabs: {},
}));

let loading: Promise<void> | null = null;
let saving: Promise<unknown> = Promise.resolve();

/** Read once; every change after it goes to the file, in order. A shell without the command has only the starter. */
export function profilesLoaded(): Promise<void> {
  loading ??= invoke<ProfilePlaces | null>("browser_profiles_read")
    .then((saved) => {
      if (saved?.profiles?.length) useProfilePlaces.setState(saved);
    })
    .catch(() => {})
    .then(() => {
      useProfilePlaces.subscribe((state) => {
        saving = saving.then(() => invoke("browser_profiles_write", { state })).catch((cause: unknown) => {
          console.error("saving the browser's profiles failed", cause);
        });
      });
    });
  return loading;
}

/** Everything changed so far is in the file. */
export const profilesSaved = (): Promise<unknown> => saving;

/** The list without those being deleted: one array per list, since a selector's answer is compared by identity. */
const liveLists = new WeakMap<BrowserProfile[], BrowserProfile[]>();
function live(profiles: BrowserProfile[]): BrowserProfile[] {
  let list = liveLists.get(profiles);
  if (!list) {
    list = profiles.filter((profile) => !profile.deleting);
    liveLists.set(profiles, list);
  }
  return list;
}

export const allProfiles = (): BrowserProfile[] => live(useProfilePlaces.getState().profiles);

function defaultOf({ profiles, defaultId }: Pick<ProfilePlaces, "profiles" | "defaultId">): BrowserProfile {
  const list = live(profiles);
  return list.find((profile) => profile.id === defaultId) ?? (list[0] as BrowserProfile);
}

export const defaultProfile = (): BrowserProfile => defaultOf(useProfilePlaces.getState());
export const useProfiles = (): BrowserProfile[] => useProfilePlaces((s) => live(s.profiles));
export const useDefaultProfile = (): BrowserProfile => useProfilePlaces(defaultOf);

/** `id` if it still names a profile. */
function known(id: string | undefined): string | null {
  return id && allProfiles().some((profile) => profile.id === id) ? id : null;
}

/** The conversation a session came from: started from, forked from, or beside. */
function parentOf(conversationId: string): string | null {
  const conversation = useApp.getState().conversations[conversationId];
  return conversation?.startedFrom ?? conversation?.forkedFrom?.conversationId ?? conversation?.sideOf ?? null;
}

/**
 * The profile a session is on, which its new tabs and an agent's tabs in it open on: its own pick, else its
 * parent's, else the default. A parent whose profile is gone has the default, and so does its child.
 */
export function sessionProfileId(conversationId: string): string {
  const { sessions } = useProfilePlaces.getState();
  const seen = new Set<string>();
  for (let at: string | null = conversationId; at && !seen.has(at); at = parentOf(at)) {
    seen.add(at);
    if (sessions[at]) return known(sessions[at]) ?? defaultProfile().id;
  }
  return defaultProfile().id;
}

/** The same, following changes. */
export function useSessionProfileId(conversationId: string): string {
  // Its parent's link comes with the conversation.
  useApp((s) => s.conversations[conversationId]);
  useProfilePlaces();
  return sessionProfileId(conversationId);
}

export function tabProfileId(tabId: string): string {
  const pinned = known(useProfilePlaces.getState().tabs[tabId]);
  if (pinned) return pinned;
  const session = sessionWith(tabId);
  return session ? sessionProfileId(session) : defaultProfile().id;
}

export function placeTab(tabId: string, profile: string): void {
  useProfilePlaces.setState(({ tabs }) => {
    const open = new Set(Object.values(useSessionTabs.getState().sessions).flatMap((session) => session.tabs.map((tab) => tab.id)));
    return { tabs: { ...Object.fromEntries(Object.entries(tabs).filter(([id]) => open.has(id))), [tabId]: profile } };
  });
}

export function placeSession(conversationId: string, profile: string): void {
  useProfilePlaces.setState(({ sessions }) => ({ sessions: { ...sessions, [conversationId]: profile } }));
}

/** The profile tab `id`'s page opens on, from now on its own and its session's. */
export function pinTabToItsProfile(id: string): string {
  const profile = tabProfileId(id);
  placeTab(id, profile);
  const session = sessionWith(id);
  if (session && !useProfilePlaces.getState().sessions[session]) placeSession(session, profile);
  return profile;
}

/** A popup's tab is on its opener's profile. */
export function shareTabProfile(opener: string, popup: string): void {
  useProfilePlaces.setState(({ tabs }) => ({ tabs: { ...tabs, [popup]: tabProfileId(opener) } }));
}

/** The session and tab in front, when a session shows. */
export function frontOf(selection = useApp.getState().selection): { session: string; tabId: string } | null {
  if (selection.type !== "conversation") return null;
  return { session: selection.id, tabId: sessionTabs(selection.id).active };
}

function frontProfileId(front: { session: string; tabId: string }): string {
  const tab = sessionTabs(front.session).tabs.find((each) => each.id === front.tabId);
  return tab?.kind === "browser" ? tabProfileId(tab.id) : sessionProfileId(front.session);
}

/** The profile the toolbar in front is on. */
export function useFrontProfile(): BrowserProfile {
  const profiles = useProfiles();
  const fallback = useDefaultProfile();
  // Each of these changes which profile that is.
  useApp((s) => s.selection);
  useSessionTabs((s) => s.sessions);
  useProfilePlaces();
  const front = frontOf();
  const id = front ? frontProfileId(front) : fallback.id;
  return profiles.find((profile) => profile.id === id) ?? fallback;
}
