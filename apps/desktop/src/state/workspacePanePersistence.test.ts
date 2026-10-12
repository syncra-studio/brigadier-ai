import assert from "node:assert/strict";
import { test } from "node:test";

const data = new Map<string, string>([
  ["brigadier.rightSidebar", JSON.stringify({ version: 1, state: { open: true, tabs: { session: "source", worker: "workers" } } })],
  ["brigadier.rightSidebarWidth", "364"],
]);
const storage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => data.get(key) ?? null,
  setItem: (key: string, value: string) => data.set(key, value),
  removeItem: (key: string) => data.delete(key),
} });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage } });
const { useWorkspacePaneState: state, WORKSPACE_PANE_KEY, WORKSPACE_PANE_WIDTH_KEY,
  openWorkspacePane, setWorkspacePaneOpen } = await import("./workspacePane");
const { cachedWidth, saveWidth } = await import("./sidebar");

test("legacy pane migration runs once, preserves its width and rehydrates both open and closed choices", async () => {
  try {
    assert.deepEqual(state.getState(), { open: true, tabs: { session: "source" } });
    assert.equal(cachedWidth(WORKSPACE_PANE_WIDTH_KEY), 364);
    assert.ok(data.has(WORKSPACE_PANE_KEY));
    openWorkspacePane("session", "files");
    saveWidth(WORKSPACE_PANE_WIDTH_KEY, 412);
    let saved = data.get(WORKSPACE_PANE_KEY)!;
    state.setState({ open: false, tabs: {} });
    data.set(WORKSPACE_PANE_KEY, saved);
    await state.persist.rehydrate();
    assert.deepEqual(state.getState(), { open: true, tabs: { session: "files" } });
    assert.equal(cachedWidth(WORKSPACE_PANE_WIDTH_KEY), 412);
    setWorkspacePaneOpen(false);
    saved = data.get(WORKSPACE_PANE_KEY)!;
    state.setState({ open: true, tabs: {} });
    data.set(WORKSPACE_PANE_KEY, saved);
    await state.persist.rehydrate();
    assert.deepEqual(state.getState(), { open: false, tabs: { session: "files" } });
    assert.equal(data.get("brigadier.rightSidebarWidth"), "364");
  } finally {
    delete (globalThis as { window?: unknown }).window;
    if (storage) Object.defineProperty(globalThis, "localStorage", storage);
    else delete (globalThis as { localStorage?: Storage }).localStorage;
  }
});
