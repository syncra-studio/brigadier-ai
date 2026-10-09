import assert from "node:assert/strict";
import { test } from "node:test";

const data = new Map<string, string>();
const storage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => data.get(key) ?? null,
  setItem: (key: string, value: string) => data.set(key, value),
  removeItem: (key: string) => data.delete(key),
} });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage } });
const { isRightSidebarKey, rightSidebarFolds, rightSidebarToggleLabel,
  useRightSidebarState: state, selectRightSidebarTab, setRightSidebarOpen, forgetRightSidebarTabs, pruneRightSidebarTabs } = await import("./rightSidebar");

const key = (change: Partial<KeyboardEvent> = {}) => ({
  code: "KeyB", metaKey: true, ctrlKey: false, altKey: true, shiftKey: false, isComposing: false,
  ...change,
}) as KeyboardEvent;

test("only Alt plus the platform command key toggles the right sidebar", () => {
  assert.equal(isRightSidebarKey(key(), true), true);
  assert.equal(isRightSidebarKey(key({ metaKey: false, ctrlKey: true }), false), true);
  for (const change of [{ altKey: false }, { shiftKey: true }, { code: "KeyS" }, { isComposing: true }, { ctrlKey: true }]) {
    assert.equal(isRightSidebarKey(key(change), true), false);
  }
  assert.equal(isRightSidebarKey(key({ metaKey: false, ctrlKey: true, getModifierState: (modifier) => modifier === "AltGraph" }), false), false);
  assert.equal(isRightSidebarKey(key(), false), false);
  assert.equal(isRightSidebarKey(key({ metaKey: false, ctrlKey: true }), true), false);
});

test("the right sidebar folds first beside an expanded left sidebar and restores when room returns", () => {
  assert.equal(rightSidebarFolds(1300, true, 288, 960), false);
  assert.equal(rightSidebarFolds(1200, true, 288, 960), true);
  assert.equal(rightSidebarFolds(1200, false, 56, 960), false);
  assert.equal(rightSidebarFolds(959, false, 0, 960), true);
  assert.equal(rightSidebarFolds(1248, true, 288, 960), false);
  assert.equal(rightSidebarToggleLabel(false), "Show right sidebar");
  assert.equal(rightSidebarToggleLabel(true), "Hide right sidebar");
});

test("open is app-wide, tabs belong to sessions, and both survive rehydration", async () => {
  try {
    assert.equal(state.getState().open, false);
    assert.equal(state.getState().tabs.newSession ?? "files", "files");
    selectRightSidebarTab("a", "workers");
    selectRightSidebarTab("b", "source");
    setRightSidebarOpen(true);
    const saved = data.get("brigadier.rightSidebar")!;
    state.setState({ open: false, tabs: {} });
    data.set("brigadier.rightSidebar", saved);
    await state.persist.rehydrate();
    assert.deepEqual(state.getState(), { open: true, tabs: { a: "workers", b: "source" } });
    setRightSidebarOpen((open) => !open);
    assert.equal(state.getState().open, false);
    assert.equal(state.getState().tabs.a, "workers");
    assert.equal(data.has("brigadier.sidebarCollapsed"), false);
    assert.equal(data.has("brigadier.sidebarWidth"), false);
    selectRightSidebarTab("a", "files");
    assert.equal(state.getState().tabs.a, undefined);
    data.set("brigadier.rightSidebar", JSON.stringify({ version: 1, state: { open: 1, tabs: { a: "browser", b: "source", c: 5 } } }));
    await state.persist.rehydrate();
    assert.deepEqual(state.getState(), { open: true, tabs: { b: "source" } });
    selectRightSidebarTab("c", "workers");
    forgetRightSidebarTabs(["b"]);
    assert.deepEqual(state.getState().tabs, { c: "workers" });
    pruneRightSidebarTabs(["a"]);
    assert.deepEqual(state.getState().tabs, {});
  } finally {
    delete (globalThis as { window?: unknown }).window;
    if (storage) Object.defineProperty(globalThis, "localStorage", storage);
    else delete (globalThis as { localStorage?: Storage }).localStorage;
  }
});
