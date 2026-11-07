// The browser's profiles on disk, in the app's data folder. The list and which profile each session and tab is on
// are the page's (src/state/browserProfilePlaces.ts), one file main only reads to guard deleting. A profile's site
// permissions are here, since main answers pages from them (permissions.ts).
import fs from "node:fs";
import path from "node:path";

import { app, BrowserWindow, session, webContents } from "electron";

import { handleFromApp } from "./appPage";
import { clearVisits, removeStoreOf } from "./browserHistory";
import { cancelDownloadsOf, removeDownloads } from "./browserDownloads";
import { replaceFile } from "./durableStorage";
import { note } from "./events";
import { listedSite, profileFolderName, profilePartition, refusalToRemove, type SavedProfiles, siteOnAsking, siteWithChoice, type SiteChoice, type SitePermission } from "./profiles";

const inData = (name: string) => path.join(app.getPath("userData"), name);
const listFile = () => inData("browser-profiles.json");
const sitesFile = () => inData("browser-sites.json");
const removedFile = () => inData("browser-profiles-removed.json");

function readJson<T>(file: string): T | null {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8")) as T;
  } catch {
    return null;
  }
}

type Sites = Record<string, Record<string, SiteChoice>>;
let sites: Sites | null = null;

function allSites(): Sites {
  sites ??= readJson<Sites>(sitesFile()) ?? {};
  return sites;
}

function keepSites(): void {
  replaceFile(sitesFile(), JSON.stringify(allSites()));
}

export function siteChoice(profileId: string, origin: string): SiteChoice | undefined {
  return allSites()[profileId]?.[origin];
}

/** The site asked: it shows in the profile's Site settings from now on. */
export function siteAsked(profileId: string, origin: string): void {
  const all = allSites();
  const known = all[profileId]?.[origin];
  all[profileId] = { ...all[profileId], [origin]: siteOnAsking(known, Date.now()) };
  keepSites();
}

export function setSiteChoice(profileId: string, origin: string, change: { camera?: SitePermission | null; microphone?: SitePermission | null; location?: SitePermission | null }): void {
  const all = allSites();
  const known = all[profileId]?.[origin];
  all[profileId] = {
    ...all[profileId],
    [origin]: siteWithChoice(known, change, Date.now()),
  };
  keepSites();
}

/** Forgets what sites asked since `sinceMs`, or all of it: they ask again. */
export function clearSiteChoices(profileId: string, sinceMs: number | null): void {
  const all = allSites();
  const kept = Object.entries(all[profileId] ?? {}).filter(([, choice]) => sinceMs !== null && choice.askedMs < sinceMs);
  all[profileId] = Object.fromEntries(kept);
  keepSites();
}

/**
 * Empties a profile's session: sign-ins, site data and caches. Its folder can't go while Chromium has the session
 * open, and Electron has no way to close one, so a deleted profile's folder goes at the next launch.
 */
async function clearProfile(profileId: string): Promise<void> {
  const profileSession = session.fromPartition(profilePartition(profileId));
  await profileSession.clearData();
  await profileSession.clearAuthCache();
  clearVisits(profileId, null);
  removeDownloads(profileId, null);
  await profileSession.cookies.flushStore();
  profileSession.flushStorageData();
  note("profile-cleared", { profileId });
}

/** Profiles on their way out: nothing new starts in one (permissions.ts answers no, and the operations below refuse). */
const deleting = new Set<string>();
export const isDeleting = (profileId: string): boolean => deleting.has(profileId);

function refuseIfDeleting(profileId: string): void {
  if (deleting.has(profileId)) throw new Error("This profile is being deleted");
}

/** The windows a page opened keep their opener's session, and the page's move to another profile can't take them. */
function closePopupsOf(profileId: string): void {
  const profileSession = session.fromPartition(profilePartition(profileId));
  for (const contents of webContents.getAllWebContents()) {
    if (contents.session === profileSession) BrowserWindow.fromWebContents(contents)?.destroy();
  }
}

async function removeProfile(profileId: string): Promise<void> {
  const refusal = refusalToRemove(profileId, readJson<SavedProfiles>(listFile()));
  if (refusal) throw new Error(refusal);
  deleting.add(profileId);
  const removed = readJson<string[]>(removedFile()) ?? [];
  // Written first: a crash during the clear still ends with the folder gone.
  replaceFile(removedFile(), JSON.stringify([...new Set([...removed, profileId])]));
  closePopupsOf(profileId);
  cancelDownloadsOf(profileId);
  await clearProfile(profileId);
  removeStoreOf(profileId);
  const all = allSites();
  delete all[profileId];
  keepSites();
  note("profile-removed", { profileId });
}

/** Sites with cookies, by domain: what the profile is signed in to at most. */
async function signInCount(profileId: string): Promise<number> {
  const cookies = await session.fromPartition(profilePartition(profileId)).cookies.get({});
  return new Set(cookies.map((cookie) => (cookie.domain ?? "").replace(/^\./, ""))).size;
}

/** Before any session opens: deleted profiles' folders, named by id only, so nothing else can be reached. */
function removeDeletedFolders(): void {
  const removed = readJson<string[]>(removedFile());
  if (!removed) return;
  for (const profileId of removed) {
    const folder = path.join(app.getPath("userData"), "Partitions", profileFolderName(profileId));
    fs.rmSync(folder, { recursive: true, force: true });
    note("profile-folder-removed", { profileId });
  }
  fs.rmSync(removedFile(), { force: true });
}

type Op = (args: Record<string, never> & { profile: string; origin: string; state: unknown; camera: SitePermission | null; microphone: SitePermission | null; location: SitePermission | null }) => unknown;

const OPS: Record<string, Op> = {
  read: () => readJson(listFile()),
  write: ({ state }) => replaceFile(listFile(), JSON.stringify(state)),
  clear: ({ profile }) => {
    refuseIfDeleting(profile);
    return clearProfile(profile);
  },
  remove: ({ profile }) => removeProfile(profile),
  counts: async ({ profile }) => {
    refuseIfDeleting(profile);
    return { signIns: await signInCount(profile) };
  },
  sites: ({ profile }) => Object.entries(allSites()[profile] ?? {}).map(([origin, choice]) => listedSite(origin, choice)),
  setSite: ({ profile, origin, camera, microphone, location }) => {
    refuseIfDeleting(profile);
    setSiteChoice(profile, origin, { camera, microphone, location });
  },
};

export function installProfileStore(): void {
  removeDeletedFolders();
  handleFromApp("brigadier:profiles", (_event, op: string, args) => {
    const run = OPS[op];
    if (!run) throw new Error(`No profile operation ${op}`);
    return run(args);
  });
}
