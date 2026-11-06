// A profile's downloads: where each file goes, its progress and a cancel. The list is in the profile's store
// (browserHistory.ts); bytes so far are only in memory, since nothing resumes a download after a restart.
import fs from "node:fs";
import path from "node:path";

import { app, type DownloadItem, type Session } from "electron";

import { likePattern, profileStore } from "./browserHistory";
import { note } from "./events";
import { mainWindow } from "./windows";

export type DownloadState = "running" | "done" | "failed" | "cancelled";
export type Download = { id: number; url: string; path: string; atMs: number; state: DownloadState; receivedBytes?: number; totalBytes?: number };

/** The app's settings main acts on itself; the page keeps them current (src/state/links.ts). */
let options = { downloadsDir: "", askDownloads: false };
export function configureDownloads(next: { downloadsDir?: string; askDownloads?: boolean }): void {
  options = { downloadsDir: next.downloadsDir ?? "", askDownloads: next.askDownloads === true };
}

/** Addresses "Save Image As…" asked for: those ask where to save, whatever the setting. */
const askFor = new Set<string>();
export function askWhereToSave(url: string): void {
  askFor.add(url);
}

const running = new Map<string, { profileId: string; id: number; item: DownloadItem }>();
const key = (profileId: string, id: number) => `${profileId}/${id}`;

/** `name` in `dir`, numbered when one is there already: a download never replaces a file. */
function freePath(dir: string, name: string): string {
  const { name: stem, ext } = path.parse(name);
  let target = path.join(dir, name);
  for (let count = 1; fs.existsSync(target); count += 1) target = path.join(dir, `${stem} (${count})${ext}`);
  return target;
}

function chosenDir(): string {
  const dir = options.downloadsDir;
  const usable = path.isAbsolute(dir) && fs.statSync(dir, { throwIfNoEntry: false })?.isDirectory() === true;
  return usable ? dir : app.getPath("downloads");
}

function tell(profileId: string, id: number, finished: { path: string; ok: boolean } | null): void {
  mainWindow()?.webContents.send("brigadier:event", "browser-download", { profile: profileId, id, finished });
}

function follow(profileId: string, item: DownloadItem, asked: boolean): void {
  const db = profileStore(profileId);
  const atMs = Date.now();
  let id: number | null = null;
  // Asked where to save, there's no path to list until the user has picked one.
  const listed = (): number | null => {
    const target = item.getSavePath();
    if (id !== null || !target) return id;
    id = Number(db.prepare("INSERT INTO downloads (url, path, at_ms, state) VALUES (?, ?, ?, 'running')").run(item.getURL(), target, atMs).lastInsertRowid);
    running.set(key(profileId, id), { profileId, id, item });
    note("download-started", { profileId, id, path: target, asked });
    return id;
  };
  listed();
  item.on("updated", () => {
    const shown = listed();
    if (shown !== null) tell(profileId, shown, null);
  });
  item.once("done", (_event, state) => {
    const shown = listed();
    note("download-done", { profileId, id: shown, state, bytes: item.getReceivedBytes() });
    if (shown === null) return;
    running.delete(key(profileId, shown));
    const final: DownloadState = state === "completed" ? "done" : state === "cancelled" ? "cancelled" : "failed";
    // The store is gone with a profile deleted meanwhile, and so is the row.
    if (stillListed(profileId, shown)) profileStore(profileId).prepare("UPDATE downloads SET state = ? WHERE id = ?").run(final, shown);
    tell(profileId, shown, final === "cancelled" ? null : { path: item.getSavePath(), ok: final === "done" });
  });
}

const forgotten = new Set<string>();
const stillListed = (profileId: string, id: number) => !forgotten.has(key(profileId, id));

/** Takes the session's downloads into the profile's folder and list. */
export function watchDownloads(session: Session, profileId: string): void {
  session.on("will-download", (_event, item) => {
    const asked = options.askDownloads || askFor.delete(item.getURL());
    const target = freePath(chosenDir(), path.basename(item.getFilename()) || "download");
    if (asked) item.setSaveDialogOptions({ defaultPath: target });
    else item.setSavePath(target);
    follow(profileId, item, asked);
  });
}

type Row = { id: number; url: string; path: string; at_ms: number; state: DownloadState };

export function downloads(profileId: string, query: string): Download[] {
  const rows = profileStore(profileId).prepare(
    "SELECT id, url, path, at_ms, state FROM downloads WHERE url LIKE ?1 ESCAPE '\\' OR path LIKE ?1 ESCAPE '\\' ORDER BY at_ms DESC, id DESC",
  ).all(likePattern(query)) as Row[];
  return rows.map((row) => {
    const live = running.get(key(profileId, row.id))?.item;
    return { id: row.id, url: row.url, path: row.path, atMs: row.at_ms, state: row.state, ...(live ? { receivedBytes: live.getReceivedBytes(), totalBytes: live.getTotalBytes() } : {}) };
  });
}

export function finishedPath(profileId: string, id: number): string {
  const row = profileStore(profileId).prepare("SELECT path FROM downloads WHERE id = ? AND state = 'done'").get(id) as { path: string } | undefined;
  if (!row || !fs.existsSync(row.path)) throw new Error("That download isn't there any more");
  return row.path;
}

export function cancelDownload(profileId: string, id: number): void {
  running.get(key(profileId, id))?.item.cancel();
}

/** Forgets downloads, the files staying: `ids`, or every finished one since `sinceMs`. One still running stays listed until it ends. */
export function removeDownloads(profileId: string, ids: readonly number[] | null, sinceMs: number | null = null): void {
  const db = profileStore(profileId);
  if (ids) {
    const remove = db.prepare("DELETE FROM downloads WHERE id = ? AND state != 'running'");
    for (const id of ids) remove.run(id);
    return;
  }
  db.prepare("DELETE FROM downloads WHERE state != 'running' AND (?1 IS NULL OR at_ms >= ?1)").run(sinceMs);
}

/** The profile is being deleted: what it still has running stops. */
export function cancelDownloadsOf(profileId: string): void {
  for (const each of [...running.values()]) {
    if (each.profileId !== profileId) continue;
    forgotten.add(key(profileId, each.id));
    each.item.cancel();
  }
}
