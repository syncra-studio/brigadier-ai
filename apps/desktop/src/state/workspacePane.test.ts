import assert from "node:assert/strict";
import { test } from "node:test";
import { isFilesSearchKey, legacyWorkspacePane, toolbarToggles, toggledWorkspacePane, workspacePaneState } from "./workspacePane";

test("sessions and project drafts end with context, files, source, terminal; chats have context, terminal", () => {
  assert.deepEqual(toolbarToggles("session"), ["context", "files", "source", "terminal"]);
  assert.deepEqual(toolbarToggles("chat"), ["context", "terminal"]);
});

test("the pane opens, closes the pressed toggle, and switches without closing", () => {
  let state = toggledWorkspacePane({ open: false, tabs: {} }, "session", "files");
  assert.equal(state.open, true);
  state = toggledWorkspacePane(state, "session", "source");
  assert.equal(state.open, true);
  assert.equal(state.tabs.session, "source");
  state = toggledWorkspacePane(state, "session", "source");
  assert.equal(state.open, false);
  state = toggledWorkspacePane(state, "session", "source");
  assert.equal(state.open, true);
});

test("stored pane tabs are validated and legacy Files/Source choices migrate", () => {
  assert.deepEqual(workspacePaneState({ open: true, tabs: { a: "source", b: "files", c: "workers" } }),
    { open: true, tabs: { a: "source", b: "files" } });
  assert.deepEqual(legacyWorkspacePane({ open: true, tabs: {} }), { open: true, tabs: {} });
  assert.deepEqual(legacyWorkspacePane({ open: true, tabs: { a: "source", b: "running" } }), { open: true, tabs: { a: "source" } });
  assert.equal(legacyWorkspacePane({ open: true, tabs: { a: "workers" } }).open, false);
  assert.equal(legacyWorkspacePane({ open: false, tabs: { a: "files" } }).open, false);
});

test("only platform command P requests Files search", () => {
  const key = { code: "KeyP", metaKey: true, ctrlKey: false, altKey: false, shiftKey: false, isComposing: false } as KeyboardEvent;
  assert.equal(isFilesSearchKey(key, true), true);
  assert.equal(isFilesSearchKey({ ...key, metaKey: false, ctrlKey: true }, false), true);
  for (const change of [{ altKey: true }, { shiftKey: true }, { isComposing: true }, { ctrlKey: true }, { code: "KeyB" }]) {
    assert.equal(isFilesSearchKey({ ...key, ...change }, true), false);
  }
});
