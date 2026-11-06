import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { atomicWrite, checkPdf, checkUrl, Documents, fileManager, fileName, openWithCommand, saveCopy, saveUnique, sourceBytes } from "./dialogFiles.ts";

const scratch = (t: test.TestContext) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "dialog-files-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  return root;
};

test("a name's last part reads as Rust's file_name does", () => {
  assert.equal(fileName("../../etc/passwd"), "passwd");
  assert.equal(fileName("notes/"), "notes");
  assert.equal(fileName("notes/."), "notes");
  for (const none of ["", ".", "..", "a/..", "/"]) assert.equal(fileName(none), null);
});

test("atomic replacement keeps the mode, and a failed rename keeps what was there with no file left over", async (t) => {
  const root = scratch(t);
  const file = path.join(root, "notes.txt");
  fs.writeFileSync(file, "original", { mode: 0o640 });
  fs.chmodSync(file, 0o640);
  await atomicWrite(file, Buffer.from("edited"), ".brigadier-save", true);
  assert.equal(fs.readFileSync(file, "utf8"), "edited");
  assert.equal(fs.statSync(file).mode & 0o777, 0o640);
  const directory = path.join(root, "directory");
  fs.mkdirSync(directory);
  fs.writeFileSync(path.join(directory, "keep"), "original");
  await assert.rejects(atomicWrite(directory, Buffer.from("no"), ".brigadier-save", true));
  assert.equal(fs.readFileSync(path.join(directory, "keep"), "utf8"), "original");
  assert.deepEqual(fs.readdirSync(root).toSorted(), ["directory", "notes.txt"]);
});

test("downloads get numbered names and never replace each other", async (t) => {
  const root = scratch(t);
  const saved = await Promise.all([1, 2, 3].map((each) => saveUnique(root, "image.png", Uint8Array.of(each))));
  assert.deepEqual(saved.map((each) => path.basename(each)).toSorted(), ["image (1).png", "image (2).png", "image.png"]);
  assert.deepEqual(saved.map((each) => [...fs.readFileSync(each)][0]).toSorted(), [1, 2, 3]);
  assert.equal(path.basename(await saveUnique(root, "image.png", Uint8Array.of(4))), "image (3).png");
  assert.equal(fs.readdirSync(root).length, 4);
});

test("a copy's source is exactly one of bytes or a file, and never a folder", async (t) => {
  const root = scratch(t);
  assert.deepEqual([...await sourceBytes([1, 2], null)], [1, 2]);
  assert.deepEqual([...await sourceBytes(Uint8Array.of(3), undefined)], [3]);
  const file = path.join(root, "a");
  fs.writeFileSync(file, "x");
  assert.equal(Buffer.from(await sourceBytes(null, file)).toString(), "x");
  await assert.rejects(sourceBytes(null, null), /exactly one/);
  await assert.rejects(sourceBytes([], file), /exactly one/);
  await assert.rejects(sourceBytes(null, root), /directory/);
  await assert.rejects(sourceBytes(null, path.join(root, "missing")), /Cannot read file/);
});

test("a copy goes to Downloads under a safe name, or where the user picks, or nowhere when they cancel", async (t) => {
  const root = scratch(t);
  const downloads = path.join(root, "made-on-demand");
  const common = { bytes: [1], source: null, downloads: () => downloads };
  assert.equal(await saveCopy({ ...common, name: "../up.png", choose: null }), path.join(downloads, "up.png"));
  let suggested: unknown = null;
  const picked = path.join(root, "picked.png");
  assert.equal(await saveCopy({ ...common, name: "..", choose: async (asked) => { suggested = asked; return picked; } }), picked);
  assert.deepEqual(suggested, { directory: downloads, name: "Untitled" });
  assert.deepEqual([...fs.readFileSync(picked)], [1]);
  assert.equal(await saveCopy({ ...common, name: "x", choose: async () => null }), null);
  await assert.rejects(saveCopy({ ...common, downloads: () => { throw new Error("none"); }, name: "x", choose: null }), /Cannot find Downloads: none/);
});

test("a document asks once, then saves to its grant; Save As asks again; cancelling writes nothing", async (t) => {
  const root = scratch(t);
  const file = path.join(root, "doc.txt");
  const asked: unknown[] = [];
  let answer: string | null = null;
  const documents = new Documents(async (suggested) => { asked.push(suggested); return answer; });
  const save = (text: string, choose = false) => documents.save({ id: "d", text, directory: root, name: "../doc.txt", choose });
  assert.equal(await save("one"), null);
  assert.equal(fs.existsSync(file), false);
  answer = file;
  assert.equal(await save("one"), file);
  assert.equal(await save("two"), file);
  assert.equal(asked.length, 2);
  assert.deepEqual(asked[1], { directory: root, name: "doc.txt" });
  assert.equal(fs.readFileSync(file, "utf8"), "two");
  const other = path.join(root, "other.txt");
  answer = other;
  assert.equal(await save("three", true), other);
  assert.equal(asked.length, 3);
  await documents.forget("d");
  answer = null;
  assert.equal(await save("four"), null);
  assert.equal(asked.length, 4);
});

test("a change outside Brigadier refuses the save and ends the grant; so does deleting the file", async (t) => {
  const root = scratch(t);
  const file = path.join(root, "doc.txt");
  let asked = 0;
  const documents = new Documents(async () => { asked++; return file; });
  const save = (text: string) => documents.save({ id: "d", text, directory: null, name: "doc.txt", choose: false });
  await save("mine");
  fs.writeFileSync(file, "theirs");
  await assert.rejects(save("mine 2"), /changed outside Brigadier/);
  assert.equal(fs.readFileSync(file, "utf8"), "theirs");
  await save("mine 3");
  assert.equal(asked, 2);
  fs.rmSync(file);
  await assert.rejects(save("mine 4"), /changed outside Brigadier/);
  assert.equal(fs.existsSync(file), false);
  await save("mine 5");
  assert.equal(asked, 3);
});

test("a failed write forgets the destination, and saves after it still run", async (t) => {
  const root = scratch(t);
  const bad = path.join(root, "directory");
  fs.mkdirSync(bad);
  const good = path.join(root, "doc.txt");
  const answers = [bad, good];
  const documents = new Documents(async () => answers.shift() ?? null);
  const save = () => documents.save({ id: "d", text: "x", directory: null, name: "doc", choose: false });
  await assert.rejects(save(), /choose another location/);
  assert.equal(await save(), good);
});

test("two saves at once take turns, so the dialog is asked once", async (t) => {
  const root = scratch(t);
  const file = path.join(root, "doc.txt");
  let asked = 0;
  const documents = new Documents(async () => { asked++; return file; });
  const both = await Promise.all(["a", "b"].map((text) => documents.save({ id: "d", text, directory: null, name: "doc", choose: false })));
  assert.deepEqual(both, [file, file]);
  assert.equal(asked, 1);
  assert.equal(fs.readFileSync(file, "utf8"), "b");
});

test("a chosen link stays a link: the grant names its target; a dangling one is refused", async (t) => {
  const root = scratch(t);
  const target = path.join(root, "target.txt");
  fs.writeFileSync(target, "old");
  const link = path.join(root, "link.txt");
  fs.symlinkSync(target, link);
  const answers = [link, path.join(root, "dangling.txt")];
  fs.symlinkSync(path.join(root, "nothing"), answers[1] as string);
  const documents = new Documents(async () => answers.shift() ?? null);
  const realRoot = fs.realpathSync(root);
  assert.equal(await documents.save({ id: "a", text: "new", directory: null, name: "x", choose: false }), path.join(realRoot, "target.txt"));
  assert.equal(fs.lstatSync(link).isSymbolicLink(), true);
  assert.equal(fs.readFileSync(target, "utf8"), "new");
  await assert.rejects(documents.save({ id: "b", text: "new", directory: null, name: "x", choose: false }), /Cannot resolve the selected file/);
});

test("only web and email links open", () => {
  for (const good of ["http://a", "HTTPS://a", "mailto:a@b"]) checkUrl(good);
  for (const bad of ["file:///etc/passwd", "javascript:alert(1)", "x-handler:go", "brigadier-ui://localhost/frame", "no-scheme"]) {
    assert.throws(() => checkUrl(bad), (error: { code: string; message: string }) => error.code === "invalid" && error.message.startsWith("only web and email links open"));
  }
});

test("only an existing PDF at an absolute path opens as one", async (t) => {
  const root = scratch(t);
  const pdf = path.join(root, "a.PDF");
  fs.writeFileSync(pdf, "%PDF");
  await checkPdf(pdf);
  const text = path.join(root, "a.txt");
  fs.writeFileSync(text, "x");
  fs.mkdirSync(path.join(root, "folder.pdf"));
  for (const bad of [text, path.join(root, "missing.pdf"), path.join(root, "folder.pdf"), path.relative(process.cwd(), pdf)]) {
    await assert.rejects(checkPdf(bad), (error: { code: string }) => error.code === "invalid");
  }
});

test("another app is named as the open crate names it", () => {
  assert.deepEqual(openWithCommand("darwin", "Terminal", "/w/-a"), { command: "/usr/bin/open", args: ["-a", "Terminal", "--", "/w/-a"] });
  assert.deepEqual(openWithCommand("linux", "gedit", "/w"), { command: "gedit", args: ["/w"] });
  assert.equal(fileManager("darwin"), "Finder");
});
