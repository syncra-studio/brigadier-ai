import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { fake } from "../testing/fakeElectron.ts";

(globalThis as { BRIGADIER_PRODUCTION?: boolean }).BRIGADIER_PRODUCTION = false;
const { importBookmarks } = await import("./browserBookmarks.ts");
const { profileFile } = await import("./browserVault.ts");

fake.userData = fs.mkdtempSync(path.join(os.tmpdir(), "brigadier-bookmarks-test-"));
test.after(() => fs.rmSync(fake.userData, { recursive: true, force: true }));

const fileOf = (profile: string) => profileFile(profile, "Brigadier Bookmarks.json");
const stored = (profile: string) => JSON.parse(fs.readFileSync(fileOf(profile), "utf8")) as {
  nextId: number;
  bookmarks: { id: number; url: string; title: string; folders: string[]; addedMs: number }[];
};

test("importing bookmarks counts what it added and what it left", async () => {
  const profile = "import-bookmarks";
  const counts = await importBookmarks(profile, [
    { url: "https://a.test/", title: "A", folders: ["Bar"], addedMs: 1000 },
    { url: "https://a.test/", title: "A again", folders: ["Bar"] },
    { url: "https://a.test/", title: "A elsewhere", folders: ["Bar", "Work"] },
    { url: "HTTP://b.test/", title: "B", folders: [] },
    { url: "ftp://c.test/", title: "C", folders: [] },
    { url: "javascript:alert(1)", title: "D", folders: [] },
  ]);
  assert.deepEqual(counts, { added: 3, skipped: 3 });

  const { nextId, bookmarks } = stored(profile);
  assert.deepEqual(bookmarks.map(({ id, title, folders }) => [id, title, folders]), [
    [1, "A", ["Bar"]],
    [2, "A elsewhere", ["Bar", "Work"]],
    [3, "B", []],
  ]);
  assert.equal(bookmarks[0]!.addedMs, 1000, "the source's date stays");
  assert.ok(bookmarks[1]!.addedMs > 1000, "with none, it is now");
  assert.equal(nextId, 4);
});

test("a second import adds only what is new", async () => {
  const profile = "import-twice";
  const items = [{ url: "https://a.test/", title: "A", folders: ["Bar"] }];
  assert.deepEqual(await importBookmarks(profile, items), { added: 1, skipped: 0 });
  assert.deepEqual(
    await importBookmarks(profile, [...items, { url: "https://b.test/", title: "B", folders: ["Bar"] }]),
    { added: 1, skipped: 1 },
  );
  assert.deepEqual(stored(profile).bookmarks.map(({ title }) => title), ["A", "B"]);
});

test("an import that adds nothing writes nothing", async () => {
  const counts = await importBookmarks("import-nothing", [{ url: "ftp://c.test/", title: "C", folders: [] }]);
  assert.deepEqual(counts, { added: 0, skipped: 1 });
  assert.equal(fs.existsSync(fileOf("import-nothing")), false);
});

test("a bookmarks file that can't be read throws, and nothing overwrites it", async () => {
  const profile = "unreadable";
  const file = fileOf(profile);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, "not json");
  await assert.rejects(importBookmarks(profile, [{ url: "https://a.test/", title: "A", folders: [] }]), SyntaxError);
  assert.equal(fs.readFileSync(file, "utf8"), "not json");
});
