import assert from "node:assert/strict";
import { test } from "node:test";

import {
  addTerminalSession,
  describeTerminalSession,
  removeTerminalSession,
  selectTerminalSession,
  terminalSessionName,
  terminalSessions,
  useTerminalSessions,
} from "./terminalSessions";

test("shell tabs use the project folder across platforms and drop numbering when only one remains", () => {
  assert.equal(
    terminalSessionName(
      "/Users/stephen/Development/brigadier-ai",
      "/tmp/session-4f2d486d",
      0,
      1,
    ),
    "brigadier-ai",
  );
  assert.equal(
    terminalSessionName(
      "C:\\code\\brigadier-ai\\",
      "C:\\worktrees\\session-4f2d486d",
      1,
      3,
    ),
    "brigadier-ai 2",
  );
  assert.equal(
    terminalSessionName(undefined, "/tmp/chat-slug", 0, 2),
    "chat-slug 1",
  );
  assert.equal(terminalSessionName(undefined, null, 0, 1), "Terminal");
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
  const id = "shell-tabs-test";
  useTerminalSessions.setState({ conversations: {} });
  const first = addTerminalSession(id);
  const second = addTerminalSession(id);
  const third = addTerminalSession(id);
  describeTerminalSession(id, second, "/tmp/session-4f2d486d");
  assert.equal(terminalSessions(id).active, third);
  removeTerminalSession(id, third);
  assert.equal(terminalSessions(id).active, second);
  selectTerminalSession(id, first);
  removeTerminalSession(id, second);
  assert.equal(terminalSessions(id).active, first);
  assert.equal(
    terminalSessionName(
      "/code/brigadier-ai",
      terminalSessions(id).sessions[0]?.cwd,
      0,
      terminalSessions(id).sessions.length,
    ),
    "brigadier-ai",
  );
  removeTerminalSession(id, first);
  assert.deepEqual(terminalSessions(id).sessions, []);
  assert.equal(terminalSessions(id).active, null);
  delete (globalThis as { localStorage?: Storage }).localStorage;
});
