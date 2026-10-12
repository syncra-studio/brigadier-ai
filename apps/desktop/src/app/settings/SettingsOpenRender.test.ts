import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Seen = {
  settings: { nav: boolean; backToApp: boolean; title: string | null };
  usage: { title: string | null; data: boolean; reading: boolean };
  computerUse: { title: string | null; status: string | null; readWaiting: boolean };
  computerUseRead: string | null;
  importFirst: { title: string | null; placeholder: boolean; loadingLine: boolean; rows: number; listWaiting: boolean };
  importAgain: { title: string | null; rows: number; listWaiting: boolean };
  storageFirst: { title: string | null; bar: boolean; scanWaiting: boolean };
  storageAgain: { title: string | null; used: boolean; bar: boolean; scanWaiting: boolean };
  inspector: { title: string | null };
  error?: string;
};

// Settings as the app lays it out, over IPC answered after a daemon-like delay: each check is
// what the first frame after a click shows (in real time: frames are what is measured).
test("settings and its pages show on the first frame, data filling in where it goes", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "settings-open.html?drive=1", "settings-open-result")) as Seen;
  assert.equal(seen.error, undefined);
  // Its code loaded at idle: the navigation and General, with no empty frame first.
  assert.deepEqual(seen.settings, { nav: true, backToApp: false, title: "General" });
  // The usage read made at idle: Usage opens filled in.
  assert.deepEqual(seen.usage, { title: "Usage", data: true, reading: false });
  // The permissions read (which starts the helper) still on its way: the page and its rows
  // show, saying it is checking; then the answer fills in.
  assert.equal(seen.computerUse.title, "Computer use");
  assert.equal(seen.computerUse.readWaiting, true);
  assert.match(seen.computerUse.status ?? "", /^Checking permissions/);
  assert.match(seen.computerUseRead ?? "", /^Finish setup/);
  // The first listing on its way: rows holding the list's place, no loading line.
  assert.deepEqual(seen.importFirst, { title: "Import Browser Data", placeholder: true, loadingLine: false, rows: 0, listWaiting: true });
  // Opened again: the browsers as last listed at once, while it lists them again.
  assert.deepEqual(seen.importAgain, { title: "Import Browser Data", rows: 4, listWaiting: true });
  // The first scan on its way: the bar holds its room.
  assert.deepEqual(seen.storageFirst, { title: "Storage", bar: true, scanWaiting: true });
  // Opened again: the last scan's findings at once, while it looks again.
  assert.deepEqual(seen.storageAgain, { title: "Storage", used: true, bar: true, scanWaiting: true });
  // Its code loaded once Settings was open and idle.
  assert.deepEqual(seen.inspector, { title: "Inspector" });
});
