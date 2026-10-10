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
} });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage } });
const { CHAT_TAB, newSessionTab, sessionTabs, selectTab, closeTab, reopenTab, moveTab,
  selectTabNumber, stepTab, changeSessionTab, restoreSessionTabs, useSessionTabs, useTabCloseAsk,
  replaceNewTab, replaceNewTabWithFile, openFileTab, openReviewTab } = await import("./sessionTabs");
const { editDocument, documentText, flushDocument, noteDocumentSave, documentIsSaved, documentRelativePath, discardDocument } = await import("./documentDrafts");

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

test("empty new files close without confirmation while edited files ask", () => {
  const id = newSessionTab("empty", "document");
  assert.equal(documentIsSaved(id), true);
  assert.equal(documentIsSaved(id, "/saved.txt"), false);
  closeTab("empty", id);
  assert.equal(useTabCloseAsk.getState().confirm, null);
  assert.equal(sessionTabs("empty").tabs.length, 0);
  const edited = newSessionTab("empty", "document");
  editDocument(edited, "unsaved"); flushDocument(edited);
  closeTab("empty", edited);
  assert.equal(typeof useTabCloseAsk.getState().confirm, "function");
  assert.equal(sessionTabs("empty").tabs.length, 1);
  useTabCloseAsk.setState({ confirm: null });
  editDocument(edited, ""); flushDocument(edited);
  closeTab("empty", edited);
  assert.equal(useTabCloseAsk.getState().confirm, null);
  assert.equal(sessionTabs("empty").tabs.length, 0);
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

test("restore retains URL, cwd, side conversation, order and draft; saved documents restore only inside checkout", async () => {
  const browser = newSessionTab("restore", "browser");
  const terminal = newSessionTab("restore", "terminal", "/workspace");
  const side = newSessionTab("restore", "sideChat");
  const draft = newSessionTab("restore", "document");
  editDocument(draft, "keep my text"); flushDocument(draft);
  changeSessionTab("restore", browser, (tab) => tab.kind === "browser" ? { ...tab, url: "http://localhost:3000", title: "Preview" } : tab);
  const inside = newSessionTab("restore", "document");
  noteDocumentSave(inside, "");
  changeSessionTab("restore", inside, (tab) => tab.kind === "document" ? { ...tab, savedPath: "/workspace/notes.txt", relativePath: "notes.txt" } : tab);
  const outside = newSessionTab("restore", "document");
  noteDocumentSave(outside, "");
  changeSessionTab("restore", outside, (tab) => tab.kind === "document" ? { ...tab, savedPath: "/elsewhere/notes.txt" } : tab);
  const saved = JSON.parse(data.get("brigadier.sessionTabs")!).state.sessions;
  assert.equal(JSON.stringify(saved).includes("keep my text"), false);
  const restored = restoreSessionTabs(saved, documentIsSaved).restore!;
  assert.deepEqual(restored.tabs.map((tab) => tab.id), [browser, terminal, side, draft, "file:notes.txt"]);
  assert.equal(restored.active, CHAT_TAB);
  assert.equal(documentText(draft), "keep my text");
  assert.deepEqual(restored.tabs.slice(0, 3), sessionTabs("restore").tabs.slice(0, 3));
  useSessionTabs.setState({ sessions: {} });
  data.set("brigadier.sessionTabs", JSON.stringify({ state: { sessions: saved }, version: 1 }));
  await useSessionTabs.persist.rehydrate();
  assert.equal(data.has("brigadier.document." + inside + ".saved"), false);
  await useSessionTabs.persist.rehydrate();
  assert.deepEqual(sessionTabs("restore").tabs.map((tab) => tab.id), restored.tabs.map((tab) => tab.id));
});

test("edits after saving survive restoration inside and outside a checkout", () => {
  for (const relativePath of ["notes.txt", null]) {
    const id = newSessionTab("dirty", "document");
    editDocument(id, "saved"); noteDocumentSave(id, "saved");
    changeSessionTab("dirty", id, (tab) => tab.kind === "document" ? { ...tab, savedPath: "/work/notes.txt", relativePath } : tab);
    editDocument(id, "edited after saving"); flushDocument(id);
    const restored = restoreSessionTabs({ dirty: sessionTabs("dirty") }, documentIsSaved).dirty!;
    const tab = restored.tabs.find((entry) => entry.id === id)!;
    assert.equal(tab.kind, "document");
    assert.equal(tab.kind === "document" && tab.savedPath, null);
    assert.equal(documentText(id), "edited after saving");
    discardDocument(id);
    assert.equal(data.has("brigadier.document." + id), false);
    assert.equal(data.has("brigadier.document." + id + ".saved"), false);
  }
  assert.equal(documentRelativePath("C:\\work", "c:\\work\\notes.txt"), "notes.txt");
  assert.equal(documentRelativePath("/work", "/worker/notes.txt"), null);
  assert.equal(documentRelativePath("/work/", "/work/notes.txt"), "notes.txt");
  assert.equal(documentRelativePath("/", "/notes.txt"), "notes.txt");
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
  for (const [code, kind] of [["KeyN", "document"], ["KeyS", "sideChat"]]) {
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
  const last = newSessionTab("new", "document");
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
  assert.equal(useTabCloseAsk.getState().confirm, null);
  reopenTab("new");
  assert.equal(sessionTabs("new").tabs[1]?.kind, "newTab");
});

test("New tab tools replace in place, preserving identity and opener", () => {
  for (const kind of ["terminal", "sideChat", "document", "browser"] as const) {
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
