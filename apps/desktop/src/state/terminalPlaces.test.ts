import assert from "node:assert/strict";
import { test } from "node:test";

import {
  addTab,
  closeTab,
  noteShell,
  noteShellCwd,
  noteShellExit,
  noteShellTitle,
  noteWorkerStarted,
  openWorkerTab,
  selectTab,
  tabNames,
  terminalPlace,
  undoTabClose,
  useTerminalPlaces,
} from "./terminalPlaces";

const tab = (cwd: string | null, shellTitle: string | null = null) => ({
  id: crypto.randomUUID(),
  shellTitle,
  cwd,
});

test("shell tabs use the project folder across platforms and number only the repeats", () => {
  assert.deepEqual(
    tabNames([tab("/tmp/session-4f2d486d")], "/Users/stephen/Development/brigadier-ai"),
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

test("a worker's tab is named for its task, reused when opened again, and not kept once closed", () => {
  const storage = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => storage.set(key, value),
      removeItem: (key: string) => storage.delete(key),
    },
  });
  const place = "conv:worker-tabs-test";
  useTerminalPlaces.setState({ places: {} });
  const shell = addTab(place);
  const worker = openWorkerTab(place, "task-1", "Fix the login form");
  assert.notEqual(worker, shell);
  assert.equal(terminalPlace(place).active, worker);
  assert.equal(terminalPlace(place).open, true);
  assert.deepEqual(terminalPlace(place).tabs[1], {
    id: worker,
    shellTitle: null,
    cwd: null,
    taskId: "task-1",
    title: "Fix the login form",
    started: false,
  });
  // The worker's CLI setting its own title doesn't rename the tab.
  noteShellTitle(place, worker, "Claude Code");
  assert.deepEqual(tabNames(terminalPlace(place).tabs), ["Terminal 1", "Fix the login form"]);

  // Opened again while its view is gone: the same tab, in front, opening the session again.
  noteWorkerStarted(place, worker);
  selectTab(place, shell);
  assert.equal(openWorkerTab(place, "task-1", "Fix the login form"), worker);
  assert.equal(terminalPlace(place).tabs.length, 2);
  assert.equal(terminalPlace(place).active, worker);
  assert.equal(terminalPlace(place).tabs[1]!.started, false);
  // Opened again while it shows: it stays started, so a later launch only reattaches.
  noteWorkerStarted(place, worker);
  noteShell(place, worker, "terminal-1");
  selectTab(place, shell);
  openWorkerTab(place, "task-1", "Fix the login form");
  assert.equal(terminalPlace(place).active, worker);
  assert.equal(terminalPlace(place).tabs[1]!.started, true);
  // Its session ending closes it.
  noteShellExit(place, worker);
  assert.deepEqual(
    terminalPlace(place).tabs.map((each) => each.id),
    [shell],
  );

  // Closed by the user, it hands the worker back: ⌘⇧T doesn't bring it back.
  const again = openWorkerTab(place, "task-1", "Fix the login form");
  closeTab(place, again);
  assert.equal(undoTabClose(place), false);
  closeTab(place, shell);
  assert.equal(undoTabClose(place), true);
  assert.equal(terminalPlace(place).tabs[0]!.taskId, undefined);
  delete (globalThis as { localStorage?: Storage }).localStorage;
});
