import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage } from "@/fixtures/headless";

type Row = { name: string; title: string; letter: string | null; dot: boolean; colour: string | null; struck: boolean; dimmed: boolean; icon: string | null };

test("the Files tree marks changes, folders holding them and ignored entries, and reads again when the checkout changes", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "files-tree.html", "files-tree-result", 6000)) as Record<string, Row[] | boolean | string>;
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  const top = seen.top as Row[];
  const named = (rows: Row[], name: string) => rows.find((row) => row.name === name)!;
  assert.deepEqual(top.map((row) => row.name), ["node_modules", "src", ".env", "package.json", "README.md"], "folders first, then files, each by name");
  assert.deepEqual(named(top, "README.md"), { ...named(top, "README.md"), letter: "M", colour: "text-warning", struck: false, dimmed: false });
  assert.ok(named(top, "README.md").icon, "files get their type's icon");
  assert.deepEqual(named(top, "src"), { ...named(top, "src"), letter: null, dot: true, colour: "text-destructive", struck: false }, "a folder takes its most severe change, as a dot, never struck through");
  assert.equal(named(top, "package.json").colour, null);
  for (const name of ["node_modules", ".env"]) assert.equal(named(top, name).dimmed, true, `${name} is ignored`);
  assert.match(named(top, ".env").title, /Ignored/);
  const opened = seen.opened as Row[];
  assert.deepEqual(named(opened, "new.ts"), { ...named(opened, "new.ts"), letter: "U", colour: "text-success" });
  assert.deepEqual(named(opened, "old.ts"), { ...named(opened, "old.ts"), letter: "D", colour: "text-destructive", struck: true }, "a deleted file stays listed, struck through");
  assert.deepEqual(named(opened, "util.ts"), { ...named(opened, "util.ts"), letter: "A", colour: "text-success" }, "a staged change counts");
  assert.equal(named(opened, "lib").dot, true);
  const ignored = seen.ignoredOpened as Row[];
  assert.deepEqual(ignored.map((row) => [row.name, row.dimmed]), [["node_modules", true], ["react", true], [".package-lock.json", true]], "an ignored folder is read when opened, all of it dimmed");
  const changed = seen.changed as Row[];
  assert.deepEqual(changed.map((row) => [row.name, row.letter]), [["CHANGELOG.md", "U"], ["README.md", null]], "a change heard from the daemon reads files and changes again");
  assert.equal(seen.folderReread, true, "opened ignored folders are read again too");
  assert.equal(seen.otherIgnored, true, "another checkout's change reads nothing");
});
