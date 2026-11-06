import { browserFocus, browserGo } from "@/ipc/client";
import { invoke, listen } from "@/ipc/native";
import {
  allProfiles,
  type BrowserProfile,
  defaultProfile,
  frontOf,
  HUES,
  placeSession,
  placeTab,
  profilesLoaded,
  profilesSaved,
  tabProfileId,
  useProfilePlaces,
} from "@/state/browserProfilePlaces";
import { closePage, useBrowsers } from "@/state/browsers";
import { newSessionTab, sessionTabs, useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

export * from "@/state/browserProfilePlaces";

/** The profile actions: what the toolbar and Settings do to profiles. */

/** Who is driving the tab in front, as the user knows them: `driver` on the tab, set by the browser tool's work. */
export function useFrontDriver(): string | null {
  return useSessionTabs((s) => {
    const front = frontOf();
    const tab = front && s.sessions[front.session]?.tabs.find((each) => each.id === front.tabId);
    return tab?.kind === "browser" ? tab.driver?.label ?? null : null;
  });
}

const nameOf = (id: string) => allProfiles().find((profile) => profile.id === id)?.name ?? id;

/**
 * Picks `profile` from the toolbar or a key: the tab in front reopens on it and the session's new
 * tabs follow; other tabs stay. A tab an agent drives isn't pulled from under it: a new tab opens on
 * the profile instead, and the agent's next tabs follow. A partition can't change on a live page, so
 * the tab's page is made again at its address. `keyboard`: the page had the keyboard, and keeps it.
 */
export function switchProfile(profile: string, { keyboard = false } = {}): void {
  const front = frontOf();
  if (!front || !allProfiles().some((each) => each.id === profile)) return;
  const { session, tabId } = front;
  const tab = sessionTabs(session).tabs.find((each) => each.id === tabId);
  if (tab?.kind !== "browser") {
    placeSession(session, profile);
    if (tab?.kind !== "newTab") toast(`New tabs in this session open in ${nameOf(profile)}`);
    return;
  }
  const current = tabProfileId(tabId);
  if (current === profile) {
    placeSession(session, profile);
    return;
  }
  if (tab.driver) {
    placeTab(tabId, current);
    placeSession(session, profile);
    newSessionTab(session, "newTab");
    return;
  }
  placeSession(session, profile);
  placeTab(tabId, profile);
  closePage(tabId);
  if (keyboard) void browserFocus(tabId).catch(() => {});
}

/** A name's folder: one path component a partition takes, never one an old profile had. */
export function newProfileId(name: string, taken: readonly string[], random = () => crypto.randomUUID().slice(0, 6)): string {
  const slug = name.toLowerCase().normalize("NFKD").replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 48) || "profile";
  for (;;) {
    const id = `${slug}-${random()}`;
    if (!taken.includes(id)) return id;
  }
}

/** The colour no profile has yet, else the first. */
export function nextHue(profiles: readonly BrowserProfile[]): number {
  return HUES.find((hue) => !profiles.some((profile) => profile.hue === hue)) ?? HUES[0];
}

export function createProfile(name: string, hue: number): string {
  const id = newProfileId(name, allProfiles().map((profile) => profile.id));
  useProfilePlaces.setState(({ profiles }) => ({ profiles: [...profiles, { id, name: name.trim(), hue }] }));
  return id;
}

export function changeProfile(id: string, change: Partial<Pick<BrowserProfile, "name" | "hue">>): void {
  useProfilePlaces.setState(({ profiles }) => ({ profiles: profiles.map((profile) => (profile.id === id ? { ...profile, ...change } : profile)) }));
}

export function moveProfile(id: string, to: number): void {
  useProfilePlaces.setState(({ profiles }) => {
    const moved = profiles.find((profile) => profile.id === id);
    if (!moved) return { profiles };
    const rest = profiles.filter((profile) => profile.id !== id);
    rest.splice(to, 0, moved);
    return { profiles: rest };
  });
}

export function makeDefaultProfile(id: string): void {
  useProfilePlaces.setState({ defaultId: id });
}

/** The sessions on `id`: those that opened a page on it or picked it, and still exist. */
export function sessionsOn(id: string, places: Record<string, string>, conversations: Record<string, unknown>): number {
  return Object.entries(places).filter(([session, profile]) => profile === id && session in conversations).length;
}

const browserTabsOn = (id: string) =>
  Object.values(useSessionTabs.getState().sessions)
    .flatMap((session) => session.tabs)
    .filter((tab) => tab.kind === "browser" && tabProfileId(tab.id) === id);

/** The open tabs on `id`. */
export const tabsOn = (id: string): number => browserTabsOn(id).length;

/** Whether a profile can be deleted: not the last one, nor the default, which can only be cleared. */
export function canDelete(id: string, profiles: readonly BrowserProfile[], defaultId: string): boolean {
  return profiles.length > 1 && id !== defaultId;
}

/** A tab an agent may be working was moved to another profile by a delete. Said once per tab, before its page goes. */
export type ProfileChange = { tabId: string; from: string; to: string };
const profileChangeListeners = new Set<(change: ProfileChange) => void>();

/** For the agent's side of the browser: its refs and page state don't survive the move. */
export function onProfileChanged(listener: (change: ProfileChange) => void): () => void {
  profileChangeListeners.add(listener);
  return () => profileChangeListeners.delete(listener);
}

/** The shell removes what's marked, and only then does the mark's entry leave the list. */
async function finishDeleting(id: string): Promise<void> {
  await profilesSaved();
  await invoke("browser_profile_delete", { profile: id });
  useProfilePlaces.setState(({ profiles }) => ({ profiles: profiles.filter((profile) => profile.id !== id) }));
}

/**
 * Deletes a profile. It's marked deleting with its sessions and tabs already on the default, all in
 * one save, so nothing new opens on it and a crash from here on still finishes. Then the tabs' pages
 * close and reopen there, and its data goes. The default is never the profile being deleted.
 */
export async function deleteProfile(id: string): Promise<void> {
  const { profiles, defaultId, sessions, tabs } = useProfilePlaces.getState();
  if (!canDelete(id, allProfiles(), defaultId)) return;
  const moveTo = defaultProfile().id;
  const moved = browserTabsOn(id);
  const onto = (places: Record<string, string>) =>
    Object.fromEntries(Object.entries(places).map(([key, profile]) => [key, profile === id ? moveTo : profile]));
  useProfilePlaces.setState({
    profiles: profiles.map((profile) => (profile.id === id ? { ...profile, deleting: true } : profile)),
    sessions: onto(sessions),
    tabs: { ...onto(tabs), ...Object.fromEntries(moved.map((tab) => [tab.id, moveTo])) },
  });
  for (const tab of moved) {
    for (const listener of profileChangeListeners) listener({ tabId: tab.id, from: id, to: moveTo });
    closePage(tab.id);
  }
  await finishDeleting(id);
}

/** A delete a crash cut short. */
function finishCutShortDeletes(): void {
  for (const profile of useProfilePlaces.getState().profiles) {
    if (profile.deleting) {
      finishDeleting(profile.id).catch((cause: unknown) => console.error("finishing a profile's delete failed", cause));
    }
  }
}

/** What a profile holds, by its session's own store. */
export type ProfileCounts = { signIns: number };

export function profileCounts(id: string): Promise<ProfileCounts> {
  return invoke<ProfileCounts>("browser_profile_counts", { profile: id });
}

/** Clears a profile's data, and its open pages show that at once, signed out. Other profiles aren't touched. */
export async function clearProfile(id: string): Promise<void> {
  await invoke("browser_profile_clear", { profile: id });
  const { pages } = useBrowsers.getState();
  for (const tab of browserTabsOn(id)) {
    if (pages[tab.id]?.ready) void browserGo(tab.id, "reload").catch(() => {});
  }
}

export const profileKeyLabel = (index: number, mac: boolean) => (mac ? `⌥⌘${index + 1}` : `Ctrl+Shift+${index + 1}`);

/** Which profile a key press switches to, 0-based; null when it isn't one. */
export function profileKey(event: Pick<KeyboardEvent, "code" | "altKey" | "metaKey" | "ctrlKey" | "shiftKey">, mac: boolean): number | null {
  const held = mac
    ? event.altKey && event.metaKey && !event.ctrlKey && !event.shiftKey
    : event.shiftKey && event.ctrlKey && !event.altKey && !event.metaKey;
  if (!held) return null;
  // Option and Shift change the key's text, so the physical key says.
  const digit = /^(?:Digit|Numpad)([1-9])$/.exec(event.code)?.[1];
  return digit ? Number(digit) - 1 : null;
}

function switchToNumber(index: number, keyboard: boolean): boolean {
  const profile = allProfiles()[index];
  if (!profile || !useApp.getState().settings.browserEnabled || !frontOf()) return false;
  switchProfile(profile.id, { keyboard });
  return true;
}

let listening = false;

/** At the app's start: a delete left unfinished, and the switching keys, once for the app: from its own page here, and from a tab's page by the shell (main/guestKeys.ts). */
export function listenForProfileKeys(): void {
  if (listening) return;
  listening = true;
  void profilesLoaded().then(finishCutShortDeletes);
  window.addEventListener(
    "keydown",
    (event) => {
      const index = profileKey(event, useApp.getState().info?.platform === "macos");
      if (index === null || !switchToNumber(index, false)) return;
      event.preventDefault();
      event.stopPropagation();
    },
    { capture: true },
  );
  void listen<number>("browser-profile-key", ({ payload }) => {
    switchToNumber(payload, true);
  });
}
