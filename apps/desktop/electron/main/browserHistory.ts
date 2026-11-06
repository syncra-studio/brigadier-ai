// STUB until section 10's branch lands (52a2b9029): same name and call, keeps nothing past the run. Its file replaces this one.
import fs from "node:fs";
import path from "node:path";

import { app } from "electron";

import { dev } from "./identity";

export type ImportedVisit = { url: string; title: string; atMs: number };

export function importHistory(profileId: string, items: readonly ImportedVisit[]): number {
  const web = items.filter((item) => /^https?:/i.test(item.url));
  if (dev) {
    const file = path.join(app.getPath("userData"), `stub-history-${profileId}.json`);
    const before = fs.existsSync(file) ? (JSON.parse(fs.readFileSync(file, "utf8")) as ImportedVisit[]) : [];
    const known = new Set(before.map((item) => `${item.url}|${item.atMs}`));
    const added = web.filter((item) => !known.has(`${item.url}|${item.atMs}`));
    fs.writeFileSync(file, JSON.stringify([...before, ...added]));
    return added.length;
  }
  return web.length;
}
