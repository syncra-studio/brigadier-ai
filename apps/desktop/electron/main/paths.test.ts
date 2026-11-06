import assert from "node:assert/strict";
import test from "node:test";

import { chromiumFolder, folderArgs } from "./paths.ts";

test("on macOS Chromium's folder is the bundle id's, never the daemon's Application Support/Brigadier", () => {
  const appData = "/Users/me/Library/Application Support";
  assert.equal(chromiumFolder("darwin", appData, "ai.brigadier.app"), `${appData}/ai.brigadier.app`);
  assert.notEqual(chromiumFolder("darwin", appData, "ai.brigadier.app"), `${appData}/Brigadier`);
});

test("elsewhere Chromium's default isn't the daemon's folder, so it stays", () => {
  assert.equal(chromiumFolder("win32", "C:\\Users\\me\\AppData\\Roaming", "ai.brigadier.app"), null);
  assert.equal(chromiumFolder("linux", "/home/me/.config", "ai.brigadier.app"), null);
});

const isFolder = (path: string) => path === "/w/repo" || path === "/w/data";

test("a folder named on the command line is opened; the data folder's value is not", () => {
  assert.deepEqual(folderArgs(["--brigadier-data-dir", "/w/data", "repo", "--smoke", "missing"], "/w", isFolder), ["/w/repo"]);
});
