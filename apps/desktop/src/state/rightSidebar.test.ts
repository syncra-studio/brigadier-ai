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
const { isRightSidebarTab, rightSidebarFolds, useRightSidebarState: state, selectRightSidebarTab,
  setRightSidebarOpen, forgetRightSidebarTabs, pruneRightSidebarTabs } = await import("./rightSidebar");

test("the right sidebar folds first beside an expanded left sidebar", () => {
  assert.equal(rightSidebarFolds(1300, true, 288, 960), false);
  assert.equal(rightSidebarFolds(1200, true, 288, 960), true);
  assert.equal(rightSidebarFolds(1200, false, 56, 960), false);
  assert.equal(rightSidebarFolds(959, false, 0, 960), true);
});

test("only Workers survives launch; old Files, Source and Running never reopen the right panel", async () => {
  try {
    assert.equal(isRightSidebarTab("workers"), true);
    for (const tab of ["files", "source", "running", "plan", "browser"]) assert.equal(isRightSidebarTab(tab), false);
    selectRightSidebarTab("a", "workers");
    selectRightSidebarTab("b", "plan");
    setRightSidebarOpen(true);
    const saved = data.get("brigadier.rightSidebar")!;
    state.setState({ open: false, tabs: {} });
    data.set("brigadier.rightSidebar", saved);
    await state.persist.rehydrate();
    assert.deepEqual(state.getState(), { open: true, tabs: { a: "workers" } });
    setRightSidebarOpen(false);
    assert.equal(state.getState().open, false);
    for (const tab of ["files", "source", "running", "plan"]) {
      data.set("brigadier.rightSidebar", JSON.stringify({ version: 1, state: { open: true, tabs: { a: tab } } }));
      await state.persist.rehydrate();
      assert.deepEqual(state.getState(), { open: false, tabs: {} });
    }
    selectRightSidebarTab("a", "workers");
    selectRightSidebarTab("b", "workers");
    forgetRightSidebarTabs(["b"]);
    assert.deepEqual(state.getState().tabs, { a: "workers" });
    pruneRightSidebarTabs([]);
    assert.deepEqual(state.getState().tabs, {});
  } finally {
    delete (globalThis as { window?: unknown }).window;
    if (storage) Object.defineProperty(globalThis, "localStorage", storage);
    else delete (globalThis as { localStorage?: Storage }).localStorage;
  }
});
