// Site storage an import left for this start: a session Chromium has open holds its folders, so they wait until none is.
import fs from "node:fs";
import path from "node:path";

const SITE_STORAGE = ["Local Storage", "IndexedDB"];

/** Moves each staged profile's folders into its partition, in place of what was there. Call before any session opens. */
export function applyStagedStorage(stagedRoot: string, partitionsRoot: string, applied: (folder: string) => void): void {
  let staged: string[];
  try {
    staged = fs.readdirSync(stagedRoot);
  } catch {
    return;
  }
  for (const folder of staged) {
    const target = path.join(partitionsRoot, folder);
    for (const name of SITE_STORAGE) {
      const from = path.join(stagedRoot, folder, name);
      if (!fs.existsSync(from)) continue;
      fs.mkdirSync(target, { recursive: true });
      fs.rmSync(path.join(target, name), { recursive: true, force: true });
      fs.renameSync(from, path.join(target, name));
    }
    fs.rmSync(path.join(stagedRoot, folder), { recursive: true, force: true });
    applied(folder);
  }
}
