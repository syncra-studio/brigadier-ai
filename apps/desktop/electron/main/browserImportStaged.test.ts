import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";

import { applyStagedStorage } from "./browserImportStaged.ts";

function scratch(t: { after: (done: () => void) => void }) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "staged-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  return { staged: path.join(root, "staged"), partitions: path.join(root, "Partitions") };
}

test("no staged folder is nothing to do", (t) => {
  const { staged, partitions } = scratch(t);
  const applied: string[] = [];
  applyStagedStorage(staged, partitions, (folder) => applied.push(folder));
  assert.deepEqual(applied, []);
  assert.equal(fs.existsSync(partitions), false);
});

test("staged folders take the place of the old ones, the rest stays, and nothing is left staged", (t) => {
  const { staged, partitions } = scratch(t);
  fs.mkdirSync(path.join(staged, "profile-a/Local Storage"), { recursive: true });
  fs.writeFileSync(path.join(staged, "profile-a/Local Storage/new"), "new");
  fs.mkdirSync(path.join(partitions, "profile-a/Local Storage"), { recursive: true });
  fs.writeFileSync(path.join(partitions, "profile-a/Local Storage/old"), "old");
  fs.mkdirSync(path.join(partitions, "profile-a/IndexedDB"), { recursive: true });
  fs.writeFileSync(path.join(partitions, "profile-a/IndexedDB/kept"), "kept");
  // A partition no session has made yet.
  fs.mkdirSync(path.join(staged, "profile-b/IndexedDB"), { recursive: true });
  fs.writeFileSync(path.join(staged, "profile-b/IndexedDB/new"), "new");

  const applied: string[] = [];
  applyStagedStorage(staged, partitions, (folder) => applied.push(folder));

  assert.deepEqual(applied.toSorted(), ["profile-a", "profile-b"]);
  assert.deepEqual(fs.readdirSync(path.join(partitions, "profile-a/Local Storage")), ["new"]);
  assert.deepEqual(fs.readdirSync(path.join(partitions, "profile-a/IndexedDB")), ["kept"]);
  assert.deepEqual(fs.readdirSync(path.join(partitions, "profile-b/IndexedDB")), ["new"]);
  assert.deepEqual(fs.readdirSync(staged), []);
});
