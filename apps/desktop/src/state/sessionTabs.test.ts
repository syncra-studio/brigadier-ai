import assert from "node:assert/strict";
import { test } from "node:test";
import { sessionTabKey } from "./sessionTabKeys";
import { localTerminalFolder } from "./terminalPaths";
import { abandonedSideChats } from "./sideChats";

const data = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => data.get(key) ?? null,
  setItem: (key: string, value: string) => data.set(key, value),
  removeItem: (key: string) => data.delete(key),
  get length() { return data.size; },
  key: (at: number) => [...data.keys()][at] ?? null,
} });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage } });
const { CHAT_TAB, newSessionTab, sessionTabs, selectTab, closeTab, closeOtherTabs, reopenTab, moveTab,
  selectTabNumber, stepTab, changeSessionTab, restoreSessionTabs, useSessionTabs,
  replaceNewTab, replaceNewTabWithFile, openFileTab, openReviewTab, showMockupTab, MOCKUP_TAB, openBrowserTab, dropBrowserTabs,
  sideChatName, sideChatTitle } = await import("./sessionTabs");
const { documentText, documentIsSaved } = await import("./documentDrafts");
const { ensurePage, migrateDrafts } = await import("./pageTabs");

test("side chat pruning preserves plain chats, unknown parents and referenced session chats", () => {
  const conversations = [
    { id: "session", kind: "session" as const, sideOf: null },
    { id: "plain", kind: "chat" as const, sideOf: null },
    ...[["orphan", "session"], ["kept", "session"], ["legacy", "plain"], ["unknown", "missing"]]
      .map(([id, sideOf]) => ({ id: id!, sideOf: sideOf!, kind: "chat" as const })),
  ];
  assert.deepEqual(abandonedSideChats([], new Set()), []);
  assert.deepEqual(abandonedSideChats(conversations.slice(2), new Set()), []);
  assert.deepEqual(abandonedSideChats(conversations, new Set(["kept"])), ["orphan"]);
});

test("the Mockup tab appears at each mockup moment without taking the front, stays closed until the next, and restores", () => {
  const session = "mockup-session";
  openFileTab(session, "src/a.ts");
  showMockupTab(session, "task-1/v1");
  assert.deepEqual(sessionTabs(session).tabs.map((tab) => tab.id), ["file:src/a.ts", MOCKUP_TAB]);
  assert.equal(sessionTabs(session).active, "file:src/a.ts");
  showMockupTab(session, "task-1/v1");
  assert.equal(sessionTabs(session).tabs.length, 2);
  closeTab(session, MOCKUP_TAB);
  closeTab(session, "file:src/a.ts");
  showMockupTab(session, "task-1/v1");
  assert.deepEqual(sessionTabs(session), { tabs: [], active: CHAT_TAB, mockup: "task-1/v1" });
  showMockupTab(session, "task-1/v2");
  assert.deepEqual(sessionTabs(session).tabs.map((tab) => tab.id), [MOCKUP_TAB]);
  selectTab(session, MOCKUP_TAB);
  const restored = restoreSessionTabs({ [session]: sessionTabs(session) })[session]!;
  assert.deepEqual(restored, { tabs: [{ kind: "mockup", id: MOCKUP_TAB, opener: CHAT_TAB }], active: MOCKUP_TAB, mockup: "task-1/v2" });
});

test("multiple tools insert after the front tab; closing returns to the opener and undo restores the position", () => {
  const a = newSessionTab("order", "terminal");
  const b = newSessionTab("order", "browser");
  selectTab("order", a);
  const c = newSessionTab("order", "sideChat");
  assert.deepEqual(sessionTabs("order").tabs.map((tab) => tab.id), [a, c, b]);
  closeTab("order", c);
  assert.equal(sessionTabs("order").active, a);
  assert.equal(reopenTab("order"), true);
  assert.deepEqual(sessionTabs("order").tabs.map((tab) => tab.id), [a, c, b]);
  moveTab("order", c, 2);
  closeTab("order", c);
  closeTab("order", b);
  reopenTab("order"); reopenTab("order");
  assert.deepEqual(sessionTabs("order").tabs.map((tab) => tab.id), [a, b, c]);
  selectTabNumber("order", 1);
  closeTab("order", CHAT_TAB);
  assert.equal(sessionTabs("order").active, CHAT_TAB);
  stepTab("order", -1);
  assert.equal(sessionTabs("order").active, c);
  stepTab("order", 1);
  assert.equal(sessionTabs("order").active, CHAT_TAB);
});

function sideNames() {
  return sessionTabs("sides").tabs.map((tab) => tab.kind === "sideChat" ? sideChatName(tab, sessionTabs("sides").tabs) : tab.kind);
}

test("side chat tabs read Side chat alone, and their title or opening number beside others", () => {
  const opened = (id: string, at: number) => changeSessionTab("sides", id, (tab) => tab.kind === "sideChat" ? { ...tab, opened: at } : tab);
  const first = newSessionTab("sides", "sideChat");
  opened(first, 1);
  assert.deepEqual(sideNames(), ["Side chat"]);
  changeSessionTab("sides", first, (tab) => tab.kind === "sideChat" ? { ...tab, title: sideChatTitle("  \n Why does   the build\tfail?\nmore") } : tab);
  assert.deepEqual(sideNames(), ["Side chat"]);
  const second = newSessionTab("sides", "sideChat");
  opened(second, 2);
  newSessionTab("sides", "terminal");
  const third = newSessionTab("sides", "sideChat");
  opened(third, 3);
  assert.deepEqual(sideNames(), ["Why does the build fail?", "Side chat 2", "terminal", "Side chat 3"]);
  moveTab("sides", third, 0);
  changeSessionTab("sides", first, (tab) => tab.kind === "sideChat" ? { ...tab, title: "" } : tab);
  assert.deepEqual(sideNames(), ["Side chat 3", "Side chat", "Side chat 2", "terminal"]);
  changeSessionTab("sides", second, (tab) => tab.kind === "sideChat" ? { ...tab, title: sideChatTitle("x".repeat(80)) } : tab);
  assert.equal(sideNames()[2], `${"x".repeat(59)}…`);
  closeTab("sides", third);
  closeTab("sides", second);
  assert.deepEqual(sideNames(), ["Side chat", "terminal"]);
});

test("restore retains URL, cwd, side conversation and order", () => {
  const browser = newSessionTab("restore", "browser");
  const terminal = newSessionTab("restore", "terminal", "/workspace");
  const side = newSessionTab("restore", "sideChat");
  const page = newSessionTab("restore", "page");
  changeSessionTab("restore", browser, (tab) => tab.kind === "browser" ? { ...tab, url: "http://localhost:3000", title: "Preview" } : tab);
  const saved = JSON.parse(data.get("brigadier.sessionTabs")!).state.sessions;
  const restored = restoreSessionTabs(saved, documentIsSaved).restore!;
  assert.deepEqual(restored.tabs.map((tab) => tab.id), [browser, terminal, side, page]);
  assert.deepEqual(restored.tabs, sessionTabs("restore").tabs.map((tab) => tab.kind === "terminal" ? { ...tab, title: "" } : tab));
});

/** An old "New file" tab as storage holds it, with its draft and what was last saved. */
function legacyDocument(id: string, text: string, saved: string | null, savedPath: string | null, relativePath: string | null) {
  data.set(`brigadier.document.${id}`, text);
  if (saved !== null) data.set(`brigadier.document.${id}.saved`, saved);
  return { kind: "document", id, opener: CHAT_TAB, name: savedPath ? savedPath.split("/").pop()! : "Untitled", savedPath, relativePath };
}

test("old New file drafts migrate: dirty ones and outside saves become pages, clean checkout saves become files", async () => {
  data.clear();
  const tabs = [
    legacyDocument("document:dirty-unsaved", "never saved text", null, null, null),
    legacyDocument("document:dirty-inside", "edited after saving", "saved", "/work/notes.md", "notes.md"),
    legacyDocument("document:dirty-outside", "edited elsewhere", "saved", "/elsewhere/plan.txt", null),
    legacyDocument("document:clean-inside", "same", "same", "/work/clean.md", "clean.md"),
    legacyDocument("document:clean-outside", "outside text", "outside text", "/elsewhere/keep.txt", null),
    legacyDocument("document:empty", "", null, null, null),
  ];
  const sessions = { legacy: { tabs: tabs as never[], active: "document:clean-inside" } };
  useSessionTabs.setState({ sessions: {} });
  data.set("brigadier.sessionTabs", JSON.stringify({ state: { sessions }, version: 1 }));
  await useSessionTabs.persist.rehydrate();
  const restored = sessionTabs("legacy");
  assert.deepEqual(restored.tabs.map((tab) => [tab.kind, tab.id]), [
    ["page", "document:dirty-unsaved"], ["page", "document:dirty-inside"], ["page", "document:dirty-outside"],
    ["file", "file:clean.md"], ["page", "document:clean-outside"], ["page", "document:empty"],
  ]);
  assert.equal(restored.active, "file:clean.md", "the tab in front follows its file");
  const pages = restored.tabs.filter((tab) => tab.kind === "page");
  assert.deepEqual(pages.map((tab) => tab.kind === "page" && [tab.title, tab.draft, tab.pageId]), [
    ["", "document:dirty-unsaved", null], ["notes", "document:dirty-inside", null], ["plan", "document:dirty-outside", null],
    ["keep", "document:clean-outside", null], ["", "document:empty", null],
  ]);
  // Every draft a page tab still waits for is kept; the file tab's draft is gone.
  for (const tab of pages) assert.ok(data.has(`brigadier.document.${tab.kind === "page" ? tab.draft : ""}`));
  assert.equal(data.has("brigadier.document.document:clean-inside"), false);
  assert.equal(data.has("brigadier.document.document:clean-inside.saved"), false);
  // Restoring again (the next launch) changes nothing and drops nothing.
  await useSessionTabs.persist.rehydrate();
  assert.deepEqual(sessionTabs("legacy"), restored);
  assert.equal(documentText("document:dirty-outside"), "edited elsewhere");

  const writes: { id: string; body: string; title: string }[] = [];
  let made = 0;
  const ipc = {
    create: async () => ({ id: `page-${++made}`, revision: "r0", content: { title: "", emoji: null, body: "" }, createdMs: 0, updatedMs: 0 }),
    write: async (id: string, content: { title: string; body: string }) => {
      if (content.body === "edited elsewhere") throw new Error("Disk full");
      writes.push({ id, body: content.body, title: content.title });
      return { type: "saved" as const, revision: "r1" };
    },
  };
  await migrateDrafts(ipc);
  assert.deepEqual(writes.map((write) => [write.title, write.body]), [
    ["", "never saved text"], ["notes", "edited after saving"], ["keep", "outside text"], ["", ""],
  ]);
  const after = sessionTabs("legacy").tabs.filter((tab) => tab.kind === "page");
  assert.deepEqual(after.map((tab) => tab.kind === "page" && [tab.pageId !== null, tab.draft ?? null]), [
    [true, null], [true, null], [false, "document:dirty-outside"], [true, null], [true, null],
  ]);
  assert.equal(data.has("brigadier.document.document:dirty-unsaved"), false, "a moved draft is removed only once its page is saved");
  assert.equal(documentText("document:dirty-outside"), "edited elsewhere", "a failed move keeps the draft");
  await useSessionTabs.persist.rehydrate();
  assert.equal(documentText("document:dirty-outside"), "edited elsewhere", "and a later launch keeps it too");
  // A second run makes no second page for the tabs already moved.
  await migrateDrafts(ipc);
  assert.equal(made, 6);
});

test("unsaved drafts whose tab was closed survive rehydration and become recent pages", async () => {
  data.clear();
  useSessionTabs.setState({ sessions: {} });
  data.set("brigadier.document.document:closed-dirty", "last unsaved words");
  data.set("brigadier.document.document:closed-dirty.saved", "previous words");
  await useSessionTabs.persist.rehydrate();
  assert.equal(documentText("document:closed-dirty"), "last unsaved words");
  const written: string[] = [];
  await migrateDrafts({
    create: async () => ({ id: "orphan-copy", revision: "r0", content: { title: "", emoji: null, body: "" }, createdMs: 0, updatedMs: 0 }),
    write: async (_id, content) => { written.push(content.body); return { type: "saved", revision: "r1" }; },
  });
  assert.deepEqual(written, ["last unsaved words"]);
  assert.equal(data.has("brigadier.document.document:closed-dirty"), false);
});

test("reopening a draft after its page was made while closed attaches the saved page", async () => {
  data.clear();
  const draft = "document:closed-during-create";
  const tab = { kind: "page" as const, id: draft, opener: CHAT_TAB, pageId: null, title: "Recovered", draft };
  useSessionTabs.setState({ sessions: { creation: { tabs: [tab], active: tab.id } } });
  data.set(`brigadier.document.${draft}`, "Kept draft text");
  let creates = 0;
  const writes: string[] = [];
  const ipc = {
    create: async () => { creates += 1; return { id: "a".repeat(32), revision: "r0", content: { title: "", emoji: null, body: "" }, createdMs: 0, updatedMs: 0 }; },
    write: async (_id: string, content: { body: string }) => { writes.push(content.body); return { type: "saved" as const, revision: "r1" }; },
  };
  const pending = ensurePage(tab, ipc);
  closeTab("creation", tab.id);
  await pending;
  assert.equal(data.has(`brigadier.document.${draft}`), false, "the draft is durable before its key goes");
  assert.equal(reopenTab("creation"), true);
  const reopened = sessionTabs("creation").tabs[0]!;
  assert.equal(reopened.kind, "page");
  if (reopened.kind !== "page") throw new Error("Expected a page");
  await ensurePage(reopened, ipc);
  const attached = sessionTabs("creation").tabs[0]!;
  assert.equal(attached.kind === "page" && attached.pageId, "a".repeat(32));
  assert.equal(creates, 1);
  assert.deepEqual(writes, ["Kept draft text"]);
});

test("a legacy draft with only its saved snapshot still preserves the saved text", () => {
  data.clear();
  data.set("brigadier.document.snapshot-only.saved", "Saved outside the checkout");
  assert.equal(documentText("snapshot-only"), "Saved outside the checkout");
  assert.equal(documentIsSaved("snapshot-only", "/elsewhere/notes.txt"), true);
  data.set("brigadier.document.snapshot-only", "");
  assert.equal(documentText("snapshot-only"), "", "an explicitly emptied draft stays empty");
  assert.equal(documentIsSaved("snapshot-only", "/elsewhere/notes.txt"), false);
});

test("OSC folders reject remote hosts and decode Windows drive paths", () => {
  assert.equal(localTerminalFolder("file://remote/work"), null);
  assert.equal(localTerminalFolder("file://localhost/work/a%20b"), "/work/a b");
  assert.equal(localTerminalFolder("file:///C:/work"), "C:/work");
  assert.equal(localTerminalFolder("https://localhost/work"), null);
  assert.equal(localTerminalFolder("not a URL"), null);
});

test("shortcut modifiers match exactly on each platform and reject composition and AltGraph", () => {
  const key = { code: "KeyT", metaKey: true, ctrlKey: false, shiftKey: false, altKey: false, isComposing: false };
  assert.deepEqual(sessionTabKey(key, true), { type: "new", kind: "newTab" });
  assert.deepEqual(sessionTabKey({ ...key, code: "KeyB", shiftKey: true }, true), { type: "new", kind: "browser" });
  for (const [code, kind] of [["KeyN", "page"], ["KeyS", "sideChat"]]) {
    assert.deepEqual(sessionTabKey({ ...key, code: code!, altKey: true }, true), { type: "new", kind });
    const ctrl = { ...key, code: code!, metaKey: false, ctrlKey: true, altKey: true };
    assert.deepEqual(sessionTabKey(ctrl, false), { type: "new", kind });
    assert.equal(sessionTabKey({ ...ctrl, getModifierState: (modifier) => modifier === "AltGraph" }, false), null);
  }
  assert.equal(sessionTabKey({ ...key, ctrlKey: true }, true), null);
  assert.equal(sessionTabKey({ ...key, isComposing: true }, true), null);
  assert.equal(sessionTabKey({ ...key, code: "KeyF", shiftKey: true }, true), null);
  assert.deepEqual(sessionTabKey({ ...key, code: "Tab", metaKey: false, ctrlKey: true, shiftKey: true }, true), { type: "step", step: -1 });
  assert.deepEqual(sessionTabKey({ ...key, code: "Digit9" }, true), { type: "number", number: 9 });
});


test("New tab inserts after the current tab, restores as New tab, and reopens after closing", async () => {
  const first = newSessionTab("new", "browser");
  const last = newSessionTab("new", "page");
  selectTab("new", first);
  const id = newSessionTab("new", "newTab");
  assert.deepEqual(sessionTabs("new").tabs.map((tab) => tab.id), [first, id, last]);
  const saved = data.get("brigadier.sessionTabs")!;
  useSessionTabs.setState({ sessions: {} });
  data.set("brigadier.sessionTabs", saved);
  await useSessionTabs.persist.rehydrate();
  assert.equal(sessionTabs("new").active, id);
  assert.equal(sessionTabs("new").tabs[1]?.kind, "newTab");
  closeTab("new", id);
  assert.equal(sessionTabs("new").active, first);
  reopenTab("new");
  assert.equal(sessionTabs("new").tabs[1]?.kind, "newTab");
});

test("New tab tools replace in place, preserving identity and opener", () => {
  for (const kind of ["terminal", "sideChat", "page", "browser"] as const) {
    const session = `replace-${kind}`;
    const opener = newSessionTab(session, "browser");
    const id = newSessionTab(session, "newTab");
    const last = newSessionTab(session, "browser");
    selectTab(session, id);
    replaceNewTab(session, id, kind, "/workspace", "https://example.com");
    assert.deepEqual(sessionTabs(session).tabs.map((tab) => tab.id), [opener, id, last]);
    assert.equal(sessionTabs(session).active, id);
    assert.equal(sessionTabs(session).tabs[1]?.kind, kind);
    assert.equal(sessionTabs(session).tabs[1]?.opener, opener);
    const restored = restoreSessionTabs({ [session]: sessionTabs(session) })[session]!;
    assert.deepEqual(restored, sessionTabs(session));
    // A stale callback must not replace a tool, especially a document with unsaved edits.
    replaceNewTab(session, id, "terminal");
    assert.equal(sessionTabs(session).tabs[1]?.kind, kind);
  }
});

test("Review replaces a placeholder in place, or consumes it and selects the existing Review", () => {
  const first = newSessionTab("review-new", "browser");
  const placeholder = newSessionTab("review-new", "newTab");
  const last = newSessionTab("review-new", "browser");
  openReviewTab("review-new", { type: "all" }, placeholder);
  assert.deepEqual(sessionTabs("review-new").tabs.map((tab) => tab.id), [first, "review", last]);
  assert.equal(sessionTabs("review-new").active, "review");
  selectTab("review-new", last);
  const duplicate = newSessionTab("review-new", "newTab");
  openReviewTab("review-new", { type: "all" }, duplicate);
  assert.deepEqual(sessionTabs("review-new").tabs.map((tab) => tab.id), [first, "review", last]);
  assert.equal(sessionTabs("review-new").active, "review");
});

test("Find file replaces its originating New tab without consuming an unrelated preview; duplicates focus the existing file", () => {
  openFileTab("find-new", "preview.ts", { preview: true });
  const placeholder = newSessionTab("find-new", "newTab");
  const last = newSessionTab("find-new", "browser");
  assert.equal(replaceNewTabWithFile("find-new", placeholder, "picked.ts"), true);
  assert.deepEqual(sessionTabs("find-new").tabs.map((tab) => tab.id), ["file:preview.ts", "file:picked.ts", last]);
  assert.equal(sessionTabs("find-new").active, "file:picked.ts");
  const duplicate = newSessionTab("find-new", "newTab");
  assert.equal(replaceNewTabWithFile("find-new", duplicate, "picked.ts"), true);
  assert.deepEqual(sessionTabs("find-new").tabs.map((tab) => tab.id), ["file:preview.ts", "file:picked.ts", last]);
  assert.equal(replaceNewTabWithFile("find-new", duplicate, "missing.ts"), false);
});

test("Cmd+T opens New tab; the native Ctrl+Backquote event opens Terminal", () => {
  const key = { code: "KeyT", metaKey: true, ctrlKey: false, shiftKey: false, altKey: false, isComposing: false };
  assert.deepEqual(sessionTabKey(key, true), { type: "new", kind: "newTab" });
  assert.deepEqual(sessionTabKey({ ...key, metaKey: false, ctrlKey: true }, false), { type: "new", kind: "newTab" });
  const terminal = { ...key, code: "Backquote", metaKey: false, ctrlKey: true };
  for (const mac of [true, false]) {
    assert.deepEqual(sessionTabKey(terminal, mac), { type: "new", kind: "terminal" });
    for (const modifier of ["shiftKey", "altKey", "metaKey"])
      assert.equal(sessionTabKey({ ...terminal, [modifier]: true }, mac), null);
  }
});

test("turning the browser off drops every Browser tab, and none can be reopened", () => {
  const id = "browser-off";
  const terminal = newSessionTab(id, "terminal");
  const page = openBrowserTab(id, "http://localhost:5173/", true);
  assert.deepEqual(sessionTabs(id).tabs.find((tab) => tab.id === page), {
    kind: "browser", id: page, opener: terminal, url: "http://localhost:5173/", title: "", agent: true,
  });
  const closed = openBrowserTab(id, "https://example.com/");
  closeTab(id, closed);
  assert.equal(sessionTabs(id).active, page);
  dropBrowserTabs();
  assert.deepEqual(sessionTabs(id).tabs.map((tab) => tab.id), [terminal]);
  // The tab in front was a Browser tab: the one that opened it shows.
  assert.equal(sessionTabs(id).active, terminal);
  assert.equal(reopenTab(id), false);
  closeTab(id, terminal);
});

test("closing the other tabs keeps Chat and the tab in front; from Chat it leaves Chat alone, and they reopen", () => {
  newSessionTab("others", "browser");
  const second = newSessionTab("others", "terminal");
  newSessionTab("others", "sideChat");
  selectTab("others", second);
  closeOtherTabs("others", sessionTabs("others").active);
  assert.deepEqual(sessionTabs("others").tabs.map((tab) => tab.id), [second]);
  assert.equal(sessionTabs("others").active, second);
  selectTab("others", CHAT_TAB);
  closeOtherTabs("others", sessionTabs("others").active);
  assert.deepEqual(sessionTabs("others").tabs, [], "the top bar is a title again");
  assert.equal(sessionTabs("others").active, CHAT_TAB);
  assert.equal(reopenTab("others"), true);
  assert.deepEqual(sessionTabs("others").tabs.map((tab) => tab.id), [second]);
});
