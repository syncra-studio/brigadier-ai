import assert from "node:assert/strict";
import { test } from "node:test";

import {
  addTab,
  closeTab,
  emitTerminalOutput,
  noteShell,
  noteShellCwd,
  selectTab,
  tabNames,
  terminalPlace,
  useTerminalPlaces,
} from "./terminalPlaces";

const tab = (cwd: string | null, shellTitle: string | null = null) => ({
  id: crypto.randomUUID(),
  shellTitle,
  cwd,
});

test("shell tabs use the project folder across platforms and number only the repeats", () => {
  assert.deepEqual(
    tabNames([tab("/tmp/session-4f2d486d")], "/Users/me/Development/brigadier-ai"),
    ["brigadier-ai"],
  );
  assert.deepEqual(
    tabNames(
      [tab("C:\\worktrees\\a"), tab("C:\\worktrees\\b"), tab(null)],
      "C:\\code\\brigadier-ai\\",
    ),
    ["brigadier-ai", "brigadier-ai 2", "brigadier-ai 3"],
  );
  assert.deepEqual(tabNames([tab("/tmp/chat-slug"), tab("/tmp/chat-slug")]), [
    "chat-slug",
    "chat-slug 2",
  ]);
  assert.deepEqual(tabNames([tab("/tmp/chat-slug", "vim notes.md"), tab(null)]), [
    "vim notes.md",
    "Terminal 2",
  ]);
});

test("closing a selected tab picks its previous neighbour; inactive close preserves selection and tabs renumber", () => {
  const storage = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => storage.set(key, value),
      removeItem: (key: string) => storage.delete(key),
    },
  });
  const place = "conv:shell-tabs-test";
  useTerminalPlaces.setState({ places: {} });
  const first = addTab(place);
  const second = addTab(place);
  const third = addTab(place);
  noteShellCwd(place, second, "/tmp/session-4f2d486d");
  assert.equal(terminalPlace(place).active, third);
  closeTab(place, third);
  assert.equal(terminalPlace(place).active, second);
  selectTab(place, first);
  closeTab(place, second);
  assert.equal(terminalPlace(place).active, first);
  assert.deepEqual(tabNames(terminalPlace(place).tabs, "/code/brigadier-ai"), ["brigadier-ai"]);
  assert.equal(terminalPlace(place).open, true);
  closeTab(place, first);
  assert.deepEqual(terminalPlace(place).tabs, []);
  assert.equal(terminalPlace(place).active, null);
  assert.equal(terminalPlace(place).open, false);
  delete (globalThis as { localStorage?: Storage }).localStorage;
});

test("a shell closed by its session's sleep keeps its tab; a real exit removes it", () => {
  const storage = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => storage.set(key, value),
      removeItem: (key: string) => storage.delete(key),
    },
  });
  const place = "conv:shell-sleep-test";
  useTerminalPlaces.setState({ places: {} });
  const shell = addTab(place);
  noteShell(place, shell, "terminal-1");
  emitTerminalOutput({ type: "exited", terminalId: "terminal-1", code: null, asleep: true });
  assert.deepEqual(terminalPlace(place).tabs.map((each) => each.id), [shell]);
  // Woken, the tab's new shell is the one a real exit ends.
  noteShell(place, shell, "terminal-2");
  emitTerminalOutput({ type: "exited", terminalId: "terminal-1", code: 0, asleep: false });
  assert.deepEqual(terminalPlace(place).tabs.map((each) => each.id), [shell]);
  emitTerminalOutput({ type: "exited", terminalId: "terminal-2", code: 0, asleep: false });
  assert.deepEqual(terminalPlace(place).tabs, []);
  delete (globalThis as { localStorage?: Storage }).localStorage;
});
