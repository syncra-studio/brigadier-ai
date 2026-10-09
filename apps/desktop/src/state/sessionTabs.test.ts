import assert from "node:assert/strict";
import { test } from "node:test";
import { sessionTabKey } from "./sessionTabKeys";

const data = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => data.get(key) ?? null,
  setItem: (key: string, value: string) => data.set(key, value),
  removeItem: (key: string) => data.delete(key),
} });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage } });
const { CHAT_TAB, newSessionTab, sessionTabs, selectTab, closeTab, reopenTab, moveTab,
  selectTabNumber, stepTab, changeSessionTab, restoreSessionTabs, useSessionTabs } = await import("./sessionTabs");
const { editDocument, documentText, flushDocument } = await import("./documentDrafts");

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

test("restore retains URL, cwd, side conversation, order and draft; saved documents restore only inside checkout", () => {
  const browser = newSessionTab("restore", "browser");
  const terminal = newSessionTab("restore", "terminal", "/workspace");
  const side = newSessionTab("restore", "sideChat");
  const draft = newSessionTab("restore", "document");
  editDocument(draft, "keep my text"); flushDocument(draft);
  changeSessionTab("restore", browser, (tab) => tab.kind === "browser" ? { ...tab, url: "http://localhost:3000", title: "Preview" } : tab);
  const inside = newSessionTab("restore", "document");
  changeSessionTab("restore", inside, (tab) => tab.kind === "document" ? { ...tab, savedPath: "/workspace/notes.txt", relativePath: "notes.txt" } : tab);
  const outside = newSessionTab("restore", "document");
  changeSessionTab("restore", outside, (tab) => tab.kind === "document" ? { ...tab, savedPath: "/elsewhere/notes.txt" } : tab);
  const saved = JSON.parse(data.get("brigadier.sessionTabs")!).state.sessions;
  assert.equal(JSON.stringify(saved).includes("keep my text"), false);
  const restored = restoreSessionTabs(saved).restore!;
  assert.deepEqual(restored.tabs.map((tab) => tab.id), [browser, terminal, side, draft, "file:notes.txt"]);
  assert.equal(restored.active, CHAT_TAB);
  assert.equal(documentText(draft), "keep my text");
  assert.deepEqual(restored.tabs.slice(0, 3), sessionTabs("restore").tabs.slice(0, 3));
  useSessionTabs.setState({ sessions: {} });
  data.set("brigadier.sessionTabs", JSON.stringify({ state: { sessions: saved }, version: 1 }));
  return useSessionTabs.persist.rehydrate();
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
