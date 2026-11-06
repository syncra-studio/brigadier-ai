// STUB until section 6's branch lands: same name and call, keeps nothing past the run. Its file replaces this one.
import fs from "node:fs";
import path from "node:path";

import { app } from "electron";

import { dev } from "./identity";

export type ImportedBookmark = { url: string; title: string; root: string; folders: string[]; added_at_ms: number | null };

export function importBookmarks(profileId: string, items: readonly ImportedBookmark[]): number {
  if (dev) {
    const file = path.join(app.getPath("userData"), `stub-bookmarks-${profileId}.json`);
    const before = fs.existsSync(file) ? (JSON.parse(fs.readFileSync(file, "utf8")) as ImportedBookmark[]) : [];
    const known = new Set(before.map((item) => item.url));
    fs.writeFileSync(file, JSON.stringify([...before, ...items.filter((item) => !known.has(item.url))]));
  }
  return items.length;
}
