import { create } from "zustand";

import { invoke } from "@/ipc/native";

/*
 * Bookmarks of the built-in browser, by profile. The store is main's (`electron/main/browserBookmarks.ts`)
 * and imports land in it.
 */

export type Bookmark = {
  id: number;
  url: string;
  title: string;
  /** Folders from the top down, such as `["Bookmarks bar", "Work"]`. */
  folders: string[];
  addedMs: number;
};

export type BookmarkEdit = { url: string; title: string; folders: string[] };

export type BookmarkFolder = { key: string; label: string; bookmarks: Bookmark[] };

export const FOLDER_SEPARATOR = " › ";

/** Goes up with every change, so a star that shows looks again. */
export const useBookmarkChanges = create<{ count: number }>(() => ({ count: 0 }));
const changed = <T>(done: T): T => {
  useBookmarkChanges.setState(({ count }) => ({ count: count + 1 }));
  return done;
};

export function browserBookmarks(profile: string): Promise<Bookmark[]> {
  return invoke<Bookmark[]>("browser_bookmarks", { profile });
}

export function addBookmark(profile: string, bookmark: BookmarkEdit): Promise<Bookmark> {
  return invoke<Bookmark>("browser_bookmark_add", { profile, bookmark }).then(changed);
}

export function updateBookmark(profile: string, id: number, bookmark: BookmarkEdit): Promise<void> {
  return invoke<void>("browser_bookmark_update", { profile, id, bookmark }).then(changed);
}

export function removeBookmark(profile: string, id: number): Promise<void> {
  return invoke<void>("browser_bookmark_remove", { profile, id }).then(changed);
}

/** Whether `url` is bookmarked in `profile`: the toolbar's star. */
export async function isBookmarked(profile: string, url: string): Promise<boolean> {
  return (await browserBookmarks(profile)).some((bookmark) => bookmark.url === url);
}

/** Bookmarks the page, in no folder, unless it is bookmarked already. */
export async function bookmarkPage(profile: string, url: string, title: string): Promise<void> {
  if (!(await isBookmarked(profile, url))) await addBookmark(profile, { url, title, folders: [] });
}

/** Removes every bookmark of the page, in whatever folder. */
export async function unbookmarkPage(profile: string, url: string): Promise<void> {
  const found = (await browserBookmarks(profile)).filter((bookmark) => bookmark.url === url);
  await Promise.all(found.map((bookmark) => removeBookmark(profile, bookmark.id)));
}

/** A folder's path as one line. A bookmark in no folder sits in "Bookmarks". */
export function folderLabel(folders: readonly string[]): string {
  return folders.join(FOLDER_SEPARATOR) || "Bookmarks";
}

/** The bookmarks whose title, address or folder holds every word of `query`, by folder, A to Z. */
export function bookmarkFolders(bookmarks: readonly Bookmark[], query: string): BookmarkFolder[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const folders = new Map<string, BookmarkFolder>();
  for (const bookmark of bookmarks) {
    const label = folderLabel(bookmark.folders);
    const text = `${bookmark.title} ${bookmark.url} ${label}`.toLowerCase();
    if (!words.every((word) => text.includes(word))) continue;
    const folder = folders.get(label);
    if (folder) folder.bookmarks.push(bookmark);
    else folders.set(label, { key: label, label, bookmarks: [bookmark] });
  }
  return [...folders.values()]
    .toSorted((a, b) => a.label.localeCompare(b.label))
    .map((folder) => ({
      ...folder,
      bookmarks: folder.bookmarks.toSorted((a, b) => (a.title || a.url).localeCompare(b.title || b.url)),
    }));
}

/** What the user typed as a bookmark's address: the built-in browser opens http and https only. */
export function bookmarkAddress(typed: string): string | null {
  const text = typed.trim();
  if (!text) return null;
  try {
    const url = new URL(/^[a-z][a-z0-9+.-]*:/i.test(text) ? text : `https://${text}`);
    return (url.protocol === "https:" || url.protocol === "http:") && url.host ? url.href : null;
  } catch {
    return null;
  }
}
