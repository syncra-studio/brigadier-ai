import assert from "node:assert/strict";
import { test } from "node:test";

import { foldersOf, mentionSections, RESULT_ROWS, type MentionSources } from "@/app/conversation/mentionSearch";
import { slashTrigger } from "@/components/conversation/elements/menuMatch";

const files = [
  "README.md",
  "src/app/Composer.tsx",
  "src/app/composerKeys.ts",
  "src/lib/menu.ts",
  "docs/composer.md",
  ...Array.from({ length: 20 }, (_, n) => `src/gen/comp${n}.ts`),
];
const sources: MentionSources = {
  workers: [{ id: "t1", number: 1, title: "Compose the release notes", state: "running" }],
  files,
  folders: foldersOf(files),
  chats: [
    { id: "c1", title: "Newest chat" },
    { id: "c2", title: "Composer ideas" },
    ...Array.from({ length: 6 }, (_, n) => ({ id: `old${n}`, title: `Older chat ${n}` })),
  ],
};

test("every folder holding a file is listed once, by its path", () => {
  assert.deepEqual(foldersOf(["a/b/c.ts", "a/d.ts", "e.ts", "a/b/f.ts"]), ["a", "a/b"]);
});

test("before anything is typed the menu has a section per kind under its heading", () => {
  const sections = mentionSections("", sources);
  assert.deepEqual(sections.map((section) => section.title), ["Workers", "Files", "Folders", "Chats"]);
  const [workers, filesSection, folders, chats] = sections;
  assert.deepEqual(workers?.rows.map((row) => [row.label, row.name, row.trailing]), [["task-1", "Compose the release notes", "running"]]);
  assert.deepEqual(filesSection?.rows, []);
  assert.equal(filesSection?.note, "Type to search for files");
  // Only the top folders, by their path.
  assert.deepEqual(folders?.rows.map((row) => row.mention), [{ type: "folder", path: "docs" }, { type: "folder", path: "src" }]);
  // The newest chats first, five of them.
  assert.equal(chats?.rows.length, 5);
  assert.equal(chats?.rows[0]?.label, "Newest chat");
});

test("a kind with nothing to mention has no section (a new chat has no workers or files)", () => {
  const sections = mentionSections("", { workers: [], files: [], folders: [], chats: sources.chats });
  assert.deepEqual(sections.map((section) => section.title), ["Chats"]);
});

test("a query gives one ranked list across every kind, at most eight", () => {
  const sections = mentionSections("comp", sources);
  assert.equal(sections.length, 1);
  assert.equal(sections[0]?.title, null);
  const rows = sections[0]!.rows;
  assert.equal(rows.length, RESULT_ROWS);
  // Names starting with the query first, the shortest name first among them.
  assert.equal(rows[0]?.label, "src/gen/comp0.ts");
  // Every kind competes in the same list: the worker and the chat whose names start with it.
  const kinds = new Set(mentionSections("compose", sources)[0]!.rows.map((row) => row.mention.type));
  assert.deepEqual([...kinds].toSorted(), ["chat", "file", "task"]);
});

test("matching is fuzzy: letters in order count, after closer matches", () => {
  const rows = mentionSections("cmpky", sources)[0]!.rows;
  assert.deepEqual(rows.map((row) => row.label), ["src/app/composerKeys.ts"]);
  // A folder matches by its name, and its files by their path after it.
  const lib = mentionSections("lib", sources)[0]!.rows;
  assert.deepEqual(lib[0]?.mention, { type: "folder", path: "src/lib" });
  assert.equal(lib[1]?.label, "src/lib/menu.ts");
  // A worker by its task number too.
  assert.equal(mentionSections("task-1", sources)[0]!.rows[0]?.mention.type, "task");
  assert.deepEqual(mentionSections("zzzz", sources)[0]!.rows, []);
});

test("/ opens the commands menu where a word starts, anywhere in the text, never inside a word", () => {
  // At the start, after leading spaces, and mid-text after a space or a newline.
  assert.deepEqual(slashTrigger("/mod", "/", 4), { query: "mod", offset: 0, endOffset: 4 });
  assert.deepEqual(slashTrigger("   /mod", "/", 7), { query: "mod", offset: 3, endOffset: 7 });
  assert.deepEqual(slashTrigger("\n /", "/", 3), { query: "", offset: 2, endOffset: 3 });
  assert.deepEqual(slashTrigger("fix this /rev", "/", 13), { query: "rev", offset: 9, endOffset: 13 });
  assert.deepEqual(slashTrigger("first line\n/gr", "/", 14), { query: "gr", offset: 11, endOffset: 14 });
  // With text after the caret, which stays where it is.
  assert.deepEqual(slashTrigger("run /re now", "/", 7), { query: "re", offset: 4, endOffset: 7 });
  // Never inside a word: a path or a link.
  assert.equal(slashTrigger("fix a/b", "/", 7), null);
  assert.equal(slashTrigger("src/a", "/", 4), null);
  assert.equal(slashTrigger("see http://x/y", "/", 14), null);
  assert.equal(slashTrigger("see http://", "/", 11), null);
  // It closes at a space, so "/overnight do it" is sent, not run again.
  assert.equal(slashTrigger("/overnight do it", "/", 16), null);
  // The caret before the slash.
  assert.equal(slashTrigger("  /x", "/", 2), null);
});

test("the session's Browser tabs are a section of their own, and a query finds one by its title", () => {
  const tabs = [
    { id: "browser:1", title: "Checkout", url: "http://localhost:3000/checkout" },
    { id: "browser:2", title: "", url: "https://example.com/" },
  ];
  const sections = mentionSections("", { ...sources, tabs });
  assert.deepEqual(sections.map((section) => section.title), ["Workers", "Browser tabs", "Files", "Folders", "Chats"]);
  assert.deepEqual(sections[1]?.rows.map((row) => [row.label, row.detail, row.mention]), [
    ["Checkout", "localhost:3000/checkout", { type: "browserTab", tab: "browser:1", title: "Checkout", url: "http://localhost:3000/checkout" }],
    ["example.com", undefined, { type: "browserTab", tab: "browser:2", title: "", url: "https://example.com/" }],
  ]);
  assert.deepEqual(mentionSections("checko", { ...sources, tabs })[0]?.rows[0]?.mention.type, "browserTab");
});
