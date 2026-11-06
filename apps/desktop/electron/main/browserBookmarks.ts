// A profile's bookmarks: one file in the profile's folder, so they go with a deleted profile and stay through clearing
// its browsing data. Only the app's own page reaches these, and section 5 through importBookmarks.
import fs from "node:fs";
import path from "node:path";

import { handleFromApp } from "./appPage";
import { replaceFile } from "./durableStorage";
import { isDeleting } from "./profileStore";
import { profileFile } from "./browserVault";

export type Bookmark = {
  id: number;
  url: string;
  title: string;
  /** Folders from the top down, such as `["Bookmarks bar", "Work"]`. None for a star in the toolbar. */
  folders: string[];
  addedMs: number;
};
type Stored = { nextId: number; bookmarks: Bookmark[] };

/** What section 5 hands over. `addedMs` is the source's date when it has one. */
export type ImportedBookmark = { url: string; title: string; folders: string[]; addedMs?: number };
export type ImportedBookmarkCounts = { added: number; skipped: number };

const fileOf = (profileId: string) => profileFile(profileId, "Brigadier Bookmarks.json");

function read(profileId: string): Stored {
  if (isDeleting(profileId)) throw new Error("This profile is being deleted");
  try {
    return JSON.parse(fs.readFileSync(fileOf(profileId), "utf8")) as Stored;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return { nextId: 1, bookmarks: [] };
    throw error;
  }
}

function write(profileId: string, stored: Stored): void {
  const file = fileOf(profileId);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  replaceFile(file, JSON.stringify(stored));
}

const webAddress = (url: string) => /^https?:\/\//i.test(url);
const sameFolder = (a: readonly string[], b: readonly string[]) => a.join("\0") === b.join("\0");

/** Section 5's door: an address outside http(s), or one already in that folder, is counted and left. */
export async function importBookmarks(profileId: string, items: readonly ImportedBookmark[]): Promise<ImportedBookmarkCounts> {
  const stored = read(profileId);
  const counts: ImportedBookmarkCounts = { added: 0, skipped: 0 };
  for (const item of items) {
    if (!webAddress(item.url) || stored.bookmarks.some((one) => one.url === item.url && sameFolder(one.folders, item.folders))) {
      counts.skipped += 1;
      continue;
    }
    stored.bookmarks.push({ id: stored.nextId++, url: item.url, title: item.title, folders: item.folders, addedMs: item.addedMs ?? Date.now() });
    counts.added += 1;
  }
  if (counts.added > 0) write(profileId, stored);
  return counts;
}

type Edit = { url: string; title: string; folders: string[] };
type Args = { profile: string; id: number; bookmark: Edit };

const OPS: Record<string, (args: Args) => unknown> = {
  list: ({ profile }) => read(profile).bookmarks,
  add: ({ profile, bookmark }) => {
    if (!webAddress(bookmark.url)) throw new Error("Only web addresses can be bookmarked.");
    const stored = read(profile);
    const added = { ...bookmark, id: stored.nextId++, addedMs: Date.now() };
    stored.bookmarks.push(added);
    write(profile, stored);
    return added;
  },
  update: ({ profile, id, bookmark }) => {
    if (!webAddress(bookmark.url)) throw new Error("Only web addresses can be bookmarked.");
    const stored = read(profile);
    stored.bookmarks = stored.bookmarks.map((one) => (one.id === id ? { ...one, ...bookmark } : one));
    write(profile, stored);
  },
  remove: ({ profile, id }) => {
    const stored = read(profile);
    stored.bookmarks = stored.bookmarks.filter((one) => one.id !== id);
    write(profile, stored);
  },
};

export function installBrowserBookmarks(): void {
  handleFromApp("brigadier:bookmarks", (_event, op: string, args: Args) => {
    const run = OPS[op];
    if (!run) throw new Error(`No bookmark operation ${op}`);
    return run(args);
  });
}
