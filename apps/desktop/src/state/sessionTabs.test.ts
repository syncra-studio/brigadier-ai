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
  selectTabNumber, stepTab, changeSessionTab, restoreSessionTabs, useSessionTabs, useTabCloseAsk } = await import("./sessionTabs");
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
  assert.deepEqual(sessionTabKey(key, true), { type: "new", kind: "terminal" });
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
