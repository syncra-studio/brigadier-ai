import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import test from "node:test";

import { fake } from "../testing/fakeElectron.ts";

// windows.ts, which says where a finished download is told, reads it at load.
(globalThis as { BRIGADIER_PRODUCTION?: boolean }).BRIGADIER_PRODUCTION = false;
const { configureDownloads, downloads, removeDownloads, watchDownloads } = await import("./browserDownloads.ts");

fake.userData = fs.mkdtempSync(path.join(os.tmpdir(), "brigadier-downloads-test-"));
const folder = path.join(fake.userData, "saved");
fs.mkdirSync(folder);
test.after(() => fs.rmSync(fake.userData, { recursive: true, force: true }));

/** A DownloadItem: the save path it was given, what it was asked to show, whether it was cancelled. */
class Item extends EventEmitter {
  savePath = "";
  dialog: unknown = null;
  cancelled = false;
  readonly url: string;
  readonly filename: string;
  constructor(url: string, filename: string) {
    super();
    this.url = url;
    this.filename = filename;
  }
  getURL = () => this.url;
  getFilename = () => this.filename;
  getSavePath = () => this.savePath;
  setSavePath = (target: string) => (this.savePath = target);
  setSaveDialogOptions = (options: unknown) => (this.dialog = options);
  getReceivedBytes = () => 10;
  getTotalBytes = () => 100;
  cancel = () => (this.cancelled = true);
}

/** A profile's session, as watchDownloads sees one; `start` begins a download in it. */
function profileSession(profile: string) {
  const session = new EventEmitter();
  watchDownloads(session as never, profile);
  return (url: string, filename: string) => {
    const item = new Item(url, filename);
    session.emit("will-download", {}, item);
    return item;
  };
}

const states = (profile: string) => downloads(profile, "").map(({ path: saved, state }) => [path.basename(saved), state]);

test("a download never replaces a file: the name is numbered", () => {
  configureDownloads({ downloadsDir: folder });
  const start = profileSession("names");
  fs.writeFileSync(path.join(folder, "a.bin"), "");
  fs.writeFileSync(path.join(folder, "a (1).bin"), "");
  assert.equal(path.basename(start("https://site.test/a.bin", "a.bin").savePath), "a (2).bin");
  assert.equal(path.basename(start("https://site.test/b.tar.gz", "b.tar.gz").savePath), "b.tar.gz");
  assert.equal(path.basename(start("https://site.test/", "").savePath), "download", "no name, still a name");
  assert.equal(path.dirname(start("https://site.test/c", "../../c").savePath), folder, "a name can't leave the folder");
});

test("a running download shows its progress, and Cancel and a broken connection end it differently", () => {
  configureDownloads({ downloadsDir: folder });
  const profile = "ends";
  const start = profileSession(profile);
  const cancelled = start("https://site.test/c.bin", "c.bin");
  const broken = start("https://site.test/d.bin", "d.bin");
  assert.deepEqual(downloads(profile, "c.bin").map(({ state, receivedBytes, totalBytes }) => ({ state, receivedBytes, totalBytes })), [{ state: "running", receivedBytes: 10, totalBytes: 100 }]);

  cancelled.emit("done", {}, "cancelled");
  // Electron holds an interrupted item open to resume; nothing resumes it, so it's cancelled and ends as failed.
  broken.emit("updated", {}, "interrupted");
  assert.equal(broken.cancelled, true);
  broken.emit("done", {}, "cancelled");
  assert.deepEqual(states(profile).sort(), [["c.bin", "cancelled"], ["d.bin", "failed"]]);
  assert.equal(downloads(profile, "")[0]!.receivedBytes, undefined, "an ended one has no progress");
});

test("asked where to save and cancelled, there's no row", () => {
  configureDownloads({ downloadsDir: folder, askDownloads: true });
  const profile = "asked";
  const start = profileSession(profile);
  const item = start("https://site.test/e.bin", "e.bin");
  assert.equal(item.savePath, "", "the user picks the path");
  assert.equal(path.basename((item.dialog as { defaultPath: string }).defaultPath), "e.bin");
  item.emit("updated", {}, "progressing");
  item.emit("done", {}, "cancelled");
  assert.deepEqual(downloads(profile, ""), []);

  const picked = start("https://site.test/f.bin", "f.bin");
  picked.savePath = path.join(folder, "chosen.bin");
  picked.emit("updated", {}, "progressing");
  assert.deepEqual(states(profile), [["chosen.bin", "running"]], "listed once a path is picked");
  configureDownloads({ downloadsDir: folder });
});

test("a run's unfinished downloads read Failed at the next open, and removing keeps running ones", () => {
  configureDownloads({ downloadsDir: folder });
  const profile = "removing";
  const start = profileSession(profile);
  const done = start("https://site.test/g.bin", "g.bin");
  start("https://site.test/h.bin", "h.bin");
  done.emit("done", {}, "cancelled");
  removeDownloads(profile, null);
  assert.deepEqual(states(profile), [["h.bin", "running"]]);
  removeDownloads(profile, downloads(profile, "").map((download) => download.id));
  assert.deepEqual(states(profile), [["h.bin", "running"]], "one still running stays listed until it ends");

  // The last run's store, with a download it never finished.
  const file = path.join(fake.userData, "Partitions", "browser-profile-leftover", "brigadier-browsing.sqlite");
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const last = new DatabaseSync(file);
  last.exec("CREATE TABLE downloads (id INTEGER PRIMARY KEY, url TEXT NOT NULL, path TEXT NOT NULL, at_ms INTEGER NOT NULL, state TEXT NOT NULL)");
  last.exec("INSERT INTO downloads (url, path, at_ms, state) VALUES ('https://site.test/i', '/x/i', 1, 'running')");
  last.close();
  assert.deepEqual(states("leftover"), [["i", "failed"]]);
});
