import assert from "node:assert/strict";
import { test } from "node:test";
import { columnsFit, isFilesSearchKey, legacyWorkspacePane, toolbarToggles, workspacePaneKey, workspacePaneState } from "./workspacePane";

test("sessions and project drafts end with context, terminal, right sidebar; chats have context, terminal", () => {
  assert.deepEqual(toolbarToggles("session"), ["context", "terminal", "right-sidebar"]);
  assert.deepEqual(toolbarToggles("chat"), ["context", "terminal"]);
});

test("the right sidebar and the Workers panel open together only when both fit beside the thread", () => {
  assert.equal(columnsFit(1440, 288, 400, 320, 320), true);
  assert.equal(columnsFit(1280, 288, 400, 320, 320), false);
  assert.equal(columnsFit(1280, 48, 400, 320, 320), true);
});

const press = (code: string, keys: Partial<KeyboardEvent>) =>
  ({ code, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, isComposing: false, ...keys }) as KeyboardEvent;

test("⌥⌘B shows or hides the right sidebar, ⇧⌘E shows Files and ⌃⇧G Source, on both platforms", () => {
  assert.equal(workspacePaneKey(press("KeyB", { metaKey: true, altKey: true }), true), "toggle");
  assert.equal(workspacePaneKey(press("KeyB", { ctrlKey: true, altKey: true }), false), "toggle");
  assert.equal(workspacePaneKey(press("KeyE", { metaKey: true, shiftKey: true }), true), "files");
  assert.equal(workspacePaneKey(press("KeyE", { ctrlKey: true, shiftKey: true }), false), "files");
  assert.equal(workspacePaneKey(press("KeyG", { ctrlKey: true, shiftKey: true }), true), "source");
  assert.equal(workspacePaneKey(press("KeyG", { ctrlKey: true, shiftKey: true }), false), "source");
  // The left sidebar's ⌘B, Show pages' ⌥⌘E and a composing key are not the sidebar's.
  for (const other of [press("KeyB", { metaKey: true }), press("KeyE", { metaKey: true, altKey: true }), press("KeyE", { metaKey: true, shiftKey: true, isComposing: true }), press("KeyG", { metaKey: true, shiftKey: true })]) {
    assert.equal(workspacePaneKey(other, true), null);
  }
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
