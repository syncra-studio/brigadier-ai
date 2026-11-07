// A profile's history, in Brigadier's own store: one SQLite file in the profile's folder, so a deleted profile's
// folder takes it along, a crash included (profileStore.ts). The engine only supplies events, and the vault isn't
// here. Downloads and what Clear browsing data dates sites by share the file (browserDownloads.ts, browserClearData.ts).
import fs from "node:fs";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";

import { app, type WebContents } from "electron";

import { note } from "./events";
import { profileFolderName, profileOfStorage } from "./profiles";

const STORE_FILE = "brigadier-browsing.sqlite";
/** The most pages a list returns at once. */
const PAGE_LIMIT = 500;

const SCHEMA = `
  CREATE TABLE IF NOT EXISTS visits (id INTEGER PRIMARY KEY, url TEXT NOT NULL, title TEXT NOT NULL DEFAULT '', at_ms INTEGER NOT NULL, agent INTEGER NOT NULL DEFAULT 0);
  CREATE INDEX IF NOT EXISTS visits_at ON visits (at_ms);
  CREATE TABLE IF NOT EXISTS downloads (id INTEGER PRIMARY KEY, url TEXT NOT NULL, path TEXT NOT NULL, at_ms INTEGER NOT NULL, state TEXT NOT NULL);
  CREATE TABLE IF NOT EXISTS site_activity (origin TEXT PRIMARY KEY, last_ms INTEGER NOT NULL);
  CREATE TABLE IF NOT EXISTS cookie_changes (domain TEXT NOT NULL, path TEXT NOT NULL, name TEXT NOT NULL, last_ms INTEGER NOT NULL, PRIMARY KEY (domain, path, name));
`;

const stores = new Map<string, DatabaseSync>();

const storeFile = (profileId: string) => path.join(app.getPath("userData"), "Partitions", profileFolderName(profileId), STORE_FILE);

/** The profile's store, opened once per run. */
export function profileStore(profileId: string): DatabaseSync {
  const open = stores.get(profileId);
  if (open) return open;
  const file = storeFile(profileId);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const db = new DatabaseSync(file);
  db.exec("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;");
  db.exec(SCHEMA);
  // A download still running at the first open is one the last run left unfinished, and nothing resumes it.
  db.exec("UPDATE downloads SET state = 'failed' WHERE state = 'running'");
  stores.set(profileId, db);
  return db;
}

/** `query` as a LIKE pattern matching it anywhere, its own `%`, `_` and `\` taken literally. */
export const likePattern = (query: string) => `%${query.trim().replace(/[%_\\]/g, "\\$&")}%`;

export type Visit = { id: number; url: string; title: string; atMs: number; agent: boolean };
export type HistoryFilter = "all" | "agent" | "other";
type VisitRow = { id: number; url: string; title: string; at_ms: number; agent: number };

const webPage = (url: string) => /^https?:/i.test(url);

/** Visits newest first, whose address or title holds `query`. The id breaks ties, so paging skips nothing at the same millisecond. */
export function history(profileId: string, query: string, filter: HistoryFilter, beforeMs: number | null, beforeId: number | null, limit: number): Visit[] {
  const agent = filter === "all" ? null : filter === "agent" ? 1 : 0;
  const rows = profileStore(profileId).prepare(
    `SELECT id, url, title, at_ms, agent FROM visits
     WHERE (url LIKE ?1 ESCAPE '\\' OR title LIKE ?1 ESCAPE '\\')
       AND (?2 IS NULL OR agent = ?2)
       AND (?3 IS NULL OR at_ms < ?3 OR (at_ms = ?3 AND id < ?5))
     ORDER BY at_ms DESC, id DESC LIMIT ?4`,
  ).all(likePattern(query), agent, beforeMs, Math.min(limit, PAGE_LIMIT), beforeId ?? Number.MAX_SAFE_INTEGER) as VisitRow[];
  return rows.map((row) => ({ id: row.id, url: row.url, title: row.title, atMs: row.at_ms, agent: row.agent === 1 }));
}

export function removeVisits(profileId: string, ids: readonly number[]): void {
  const remove = profileStore(profileId).prepare("DELETE FROM visits WHERE id = ?");
  for (const id of ids) remove.run(id);
  forgetGoneVisits(profileId);
}

export function clearVisits(profileId: string, sinceMs: number | null): void {
  profileStore(profileId).prepare("DELETE FROM visits WHERE ?1 IS NULL OR at_ms >= ?1").run(sinceMs);
  forgetGoneVisits(profileId);
}

/** A page another browser showed, for section 5's import. `title` may be empty; `atMs` is ms since the Unix epoch. */
export type ImportedVisit = { url: string; title: string; atMs: number };

/**
 * Adds pages another browser showed to the profile's history, as the user's own. Ones already here at the same time
 * are skipped, so importing twice adds nothing, and so is anything but a web address. Returns how many were added.
 */
export function importHistory(profileId: string, items: readonly ImportedVisit[]): number {
  const db = profileStore(profileId);
  const known = db.prepare("SELECT 1 FROM visits WHERE url = ? AND at_ms = ?");
  const insert = db.prepare("INSERT INTO visits (url, title, at_ms, agent) VALUES (?, ?, ?, 0)");
  let added = 0;
  db.exec("BEGIN");
  try {
    for (const { url, title, atMs } of items) {
      if (!webPage(url) || known.get(url, atMs)) continue;
      insert.run(url, title, atMs);
      added += 1;
    }
    db.exec("COMMIT");
  } catch (error) {
    db.exec("ROLLBACK");
    throw error;
  }
  return added;
}

/** What a guest last put in the history. `clearedUrl`: its page when that row was removed; a reload of it mustn't bring the row back. */
type Watched = { profileId: string; agent: boolean; restored: boolean; id: number | null; url: string; clearedUrl: string | null };
const watched = new Map<number, Watched>();

function forgetGoneVisits(profileId: string): void {
  const exists = profileStore(profileId).prepare("SELECT 1 FROM visits WHERE id = ?");
  for (const guest of watched.values()) {
    if (guest.profileId !== profileId || guest.id === null || exists.get(guest.id)) continue;
    guest.id = null;
    guest.clearedUrl = guest.url;
  }
}

/** The guest shows `url` without it being a visit: it carries on the newest row of that address, or none if it was cleared. */
function carryOn(guest: Watched, url: string): void {
  const row = profileStore(guest.profileId).prepare("SELECT id FROM visits WHERE url = ? ORDER BY at_ms DESC LIMIT 1").get(url) as { id: number } | undefined;
  guest.id = row?.id ?? null;
  guest.url = url;
  guest.clearedUrl = row ? null : url;
}

function visited(guest: Watched, url: string): void {
  if (guest.restored) {
    guest.restored = false;
    if (webPage(url)) {
      carryOn(guest, url);
      return;
    }
  }
  if (!webPage(url)) {
    guest.id = null;
    guest.url = url;
    return;
  }
  if (url === guest.clearedUrl) return;
  guest.clearedUrl = null;
  const db = profileStore(guest.profileId);
  const now = Date.now();
  // The same address again, a reload or a redirect to itself, is one visit that moves up.
  if (guest.id !== null && guest.url === url) {
    db.prepare("UPDATE visits SET at_ms = ? WHERE id = ?").run(now, guest.id);
    return;
  }
  const row = db.prepare("INSERT INTO visits (url, title, at_ms, agent) VALUES (?, '', ?, ?)").run(url, now, guest.agent ? 1 : 0);
  guest.id = Number(row.lastInsertRowid);
  guest.url = url;
}

/** Records the pages a guest shows, in a tab or a popup. */
export function watchVisits(contents: WebContents): void {
  const profileId = profileOfStorage(contents.session.storagePath ?? null);
  if (!profileId) return;
  const guest: Watched = { profileId, agent: false, restored: false, id: null, url: "", clearedUrl: null };
  watched.set(contents.id, guest);
  contents.once("destroyed", () => watched.delete(contents.id));
  contents.on("did-navigate", (_event, url) => visited(guest, url));
  contents.on("did-navigate-in-page", (_event, url, isMainFrame) => {
    if (isMainFrame) visited(guest, url);
  });
  contents.on("page-title-updated", (_event, title) => {
    if (guest.id !== null && title) profileStore(profileId).prepare("UPDATE visits SET title = ? WHERE id = ?").run(title, guest.id);
  });
}

/**
 * What the tab layer knows of its guest. `agent`: an agent gave the tab its page, so its pages count as the agent's.
 * `restored`: the tab came back at a page it already had, after a restart, which is no new visit. This can arrive
 * after the first load has been recorded, so it also undoes that.
 */
export function describeGuest(webContentsId: number, { agent, restored }: { agent: boolean; restored: boolean }): void {
  const guest = watched.get(webContentsId);
  note("guest-described", { webContents: webContentsId, known: Boolean(guest), agent, restored, recorded: guest?.id ?? null });
  if (!guest) return;
  const db = profileStore(guest.profileId);
  guest.agent = agent;
  if (agent && guest.id !== null) db.prepare("UPDATE visits SET agent = 1 WHERE id = ?").run(guest.id);
  if (!restored) return;
  if (guest.id === null) {
    guest.restored = true;
    return;
  }
  db.prepare("DELETE FROM visits WHERE id = ?").run(guest.id);
  carryOn(guest, guest.url);
}

/** The profile's data is going: its store goes now, with what's in it. */
export function removeStoreOf(profileId: string): void {
  stores.get(profileId)?.close();
  const file = storeFile(profileId);
  for (const each of [file, `${file}-wal`, `${file}-shm`]) fs.rmSync(each, { force: true });
  // Its session's last events still come (the clear's own cookie changes, a closing guest's title): they land nowhere, and the file stays gone.
  const nowhere = new DatabaseSync(":memory:");
  nowhere.exec(SCHEMA);
  stores.set(profileId, nowhere);
  for (const guest of watched.values()) {
    if (guest.profileId === profileId) guest.id = null;
  }
  note("browsing-store-removed", { profileId });
}
