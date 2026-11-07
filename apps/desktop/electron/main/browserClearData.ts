// Clear browsing data, for one profile: a time range and a tick for each kind. Saved passwords and bookmarks aren't
// engine data, so nothing here reaches them. Chromium gives neither cookies nor site storage a date Electron can
// read (`Cookie` has no creation time, `clearData` no range), so the profile's store keeps its own: when each cookie
// last changed, and when each origin last had a frame. A cookie or origin last seen before this code ran counts as older.
import { session, webContents, type Session, type WebContents } from "electron";

import { handleFromApp } from "./appPage";
import { clearVisits, describeGuest, history, profileStore, removeVisits, type HistoryFilter } from "./browserHistory";
import { cancelDownload, configureDownloads, downloads, finishedPath, removeDownloads, watchDownloads } from "./browserDownloads";
import { openChecked } from "./dialogs";
import { note } from "./events";
import { clearSiteChoices } from "./profileStore";
import { profileOfStorage, profilePartition } from "./profiles";

const STORAGE_KINDS = ["backgroundFetch", "fileSystems", "indexedDB", "localStorage", "serviceWorkers", "webSQL"] as const;

export type ClearRequest = { sinceMs: number | null; history: boolean; cookies: boolean; cache: boolean; downloads: boolean; sites: boolean };

type CookieKey = { domain: string; path: string; name: string };
/** Cookie changes wait for one write: a site can set dozens at once, and this is the main thread. */
const waiting = new Map<string, Map<string, CookieKey & { lastMs: number | null }>>();
let writeSoon: ReturnType<typeof setTimeout> | null = null;

function writeCookieChanges(): void {
  if (writeSoon !== null) clearTimeout(writeSoon);
  writeSoon = null;
  for (const [profileId, changes] of waiting) {
    const db = profileStore(profileId);
    const upsert = db.prepare("INSERT INTO cookie_changes (domain, path, name, last_ms) VALUES (?, ?, ?, ?) ON CONFLICT (domain, path, name) DO UPDATE SET last_ms = excluded.last_ms");
    const remove = db.prepare("DELETE FROM cookie_changes WHERE domain = ? AND path = ? AND name = ?");
    db.exec("BEGIN");
    for (const { domain, path, name, lastMs } of changes.values()) {
      if (lastMs === null) remove.run(domain, path, name);
      else upsert.run(domain, path, name, lastMs);
    }
    db.exec("COMMIT");
  }
  waiting.clear();
}

/** What Clear needs of a profile's session from the moment it opens: its downloads and the dates of its cookies. */
export function watchProfileSession(created: Session): void {
  const profileId = profileOfStorage(created.storagePath ?? null);
  if (!profileId) return;
  watchDownloads(created, profileId);
  created.cookies.on("changed", (_event, cookie, cause, removed) => {
    // An overwrite comes as a removal and a set: the set says when it changed.
    if (removed && cause === "overwrite") return;
    const domain = cookie.domain ?? "";
    const path = cookie.path ?? "";
    const changes = waiting.get(profileId) ?? new Map<string, CookieKey & { lastMs: number | null }>();
    waiting.set(profileId, changes);
    changes.set(`${domain}|${path}|${cookie.name}`, { domain, path, name: cookie.name, lastMs: removed ? null : Date.now() });
    writeSoon ??= setTimeout(writeCookieChanges, 1000);
  });
}

const originOf = (url: string): string | null => {
  try {
    const { protocol, origin } = new URL(url);
    return protocol === "http:" || protocol === "https:" ? origin : null;
  } catch {
    return null;
  }
};

/** Dates each origin a guest's frames go to, for clearing site data by range. */
export function watchSiteActivity(contents: WebContents): void {
  const profileId = profileOfStorage(contents.session.storagePath ?? null);
  if (!profileId) return;
  contents.on("did-frame-navigate", (_event, url) => {
    const origin = originOf(url);
    if (origin) profileStore(profileId).prepare("INSERT INTO site_activity (origin, last_ms) VALUES (?, ?) ON CONFLICT (origin) DO UPDATE SET last_ms = excluded.last_ms").run(origin, Date.now());
  });
}

/** Origins with data that may have changed since `sinceMs`: those with a frame since then, and those with one now. */
function originsSince(profileId: string, profileSession: Session, sinceMs: number): string[] {
  const rows = profileStore(profileId).prepare("SELECT origin FROM site_activity WHERE last_ms >= ?").all(sinceMs) as { origin: string }[];
  const origins = new Set(rows.map((row) => row.origin));
  for (const contents of webContents.getAllWebContents()) {
    if (contents.session !== profileSession) continue;
    for (const frame of contents.mainFrame.framesInSubtree) {
      const origin = originOf(frame.url);
      if (origin) origins.add(origin);
    }
  }
  return [...origins];
}

async function clearCookiesAndSiteData(profileId: string, profileSession: Session, sinceMs: number | null): Promise<void> {
  writeCookieChanges();
  const db = profileStore(profileId);
  if (sinceMs === null) {
    await profileSession.clearData({ dataTypes: ["cookies", ...STORAGE_KINDS] });
    await profileSession.clearAuthCache();
    db.exec("DELETE FROM cookie_changes; DELETE FROM site_activity;");
  } else {
    const changed = db.prepare("SELECT domain, path, name FROM cookie_changes WHERE last_ms >= ?").all(sinceMs) as CookieKey[];
    const dated = new Set(changed.map(({ domain, path, name }) => `${domain}|${path}|${name}`));
    for (const cookie of await profileSession.cookies.get({})) {
      if (!dated.has(`${cookie.domain ?? ""}|${cookie.path ?? ""}|${cookie.name}`)) continue;
      const host = (cookie.domain ?? "").replace(/^\./, "");
      await profileSession.cookies.remove(`${cookie.secure ? "https" : "http"}://${host}${cookie.path ?? "/"}`, cookie.name);
    }
    const origins = originsSince(profileId, profileSession, sinceMs);
    if (origins.length > 0) await profileSession.clearData({ dataTypes: [...STORAGE_KINDS], origins });
    writeCookieChanges();
  }
  // Written out now: a crash right after a clear mustn't bring a sign-in back.
  await profileSession.cookies.flushStore();
  profileSession.flushStorageData();
}

async function clearBrowsingData(profileId: string, clear: ClearRequest): Promise<void> {
  const profileSession = session.fromPartition(profilePartition(profileId));
  if (clear.history) clearVisits(profileId, clear.sinceMs);
  if (clear.downloads) removeDownloads(profileId, null, clear.sinceMs);
  if (clear.sites) clearSiteChoices(profileId, clear.sinceMs);
  if (clear.cookies) await clearCookiesAndSiteData(profileId, profileSession, clear.sinceMs);
  // Chromium's cache has no range Electron offers: any range clears all of it, as the form says.
  if (clear.cache) await profileSession.clearData({ dataTypes: ["cache"] });
  note("browsing-data-cleared", { profileId, ...clear });
}

/** Sites with cookies, by domain. */
async function siteCount(profileId: string): Promise<number> {
  const cookies = await session.fromPartition(profilePartition(profileId)).cookies.get({});
  return new Set(cookies.map((cookie) => (cookie.domain ?? "").replace(/^\./, ""))).size;
}

type Args = { profile: string; query: string; filter: HistoryFilter; beforeMs: number | null; beforeId: number | null; limit: number; ids: number[] | null; id: number; clear: ClearRequest; webContents: number; agent: boolean; restored: boolean; downloadsDir?: string; askDownloads?: boolean };

const OPS: Record<string, (args: Args) => unknown> = {
  history: ({ profile, query, filter, beforeMs, beforeId, limit }) => history(profile, query, filter, beforeMs, beforeId, limit),
  historyRemove: ({ profile, ids }) => removeVisits(profile, ids ?? []),
  downloads: ({ profile, query }) => downloads(profile, query),
  downloadOpen: ({ profile, id }) => openChecked(finishedPath(profile, id), null),
  downloadCancel: ({ profile, id }) => cancelDownload(profile, id),
  downloadsRemove: ({ profile, ids }) => removeDownloads(profile, ids),
  clear: ({ profile, clear }) => clearBrowsingData(profile, clear),
  siteCount: ({ profile }) => siteCount(profile),
  configure: (args) => configureDownloads(args),
  guest: ({ webContents: id, agent, restored }) => describeGuest(id, { agent, restored }),
};

export function installBrowserData(): void {
  handleFromApp("brigadier:browser-data", (_event, op: string, args: Args) => {
    const run = OPS[op];
    if (!run) throw new Error(`No browser data operation ${op}`);
    return run(args);
  });
}
