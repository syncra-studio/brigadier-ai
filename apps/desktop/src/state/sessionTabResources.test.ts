import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";
import { mockIPC } from "@tauri-apps/api/mocks";

import type { RequestOf } from "@/ipc/client";

const data = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => data.get(key) ?? null,
  setItem: (key: string, value: string) => data.set(key, value),
  removeItem: (key: string) => data.delete(key),
} });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage } });
const { closeTab, newSessionTab } = await import("./sessionTabs");
const { endMainTerminal, openMainTerminal } = await import("./sessionTabResources");

type Sent = RequestOf<"openTerminal"> | RequestOf<"closeTerminal">;
let sent: Sent[];
// The daemon's shells by session: an open without `fresh` reattaches to a running one.
let running: Map<string, string>;
let next: number;

beforeEach(() => {
  sent = [];
  running = new Map();
  next = 0;
  mockIPC((_command, payload) => {
    const req = (payload as { request: Sent }).request;
    sent.push(req);
    if (req.method === "closeTerminal") {
      for (const [session, id] of running) if (id === req.terminalId) running.delete(session);
      return { method: req.method };
    }
    const session = req.sessionId!;
    const current = running.get(session);
    const id = !req.fresh && current ? current : `shell-${++next}`;
    running.set(session, id);
    return { method: req.method, terminal: { id, shell: "/bin/zsh", cwd: "/tmp", scrollback: current && !req.fresh ? "startup\r\n" : "" } };
  });
});

const opens = () => sent.filter((req) => req.method === "openTerminal");

test("a new terminal tab uses the shell it started, so its startup output streams once", async () => {
  const tab = newSessionTab("new", "terminal");
  const terminal = await openMainTerminal("new", tab, 80, 24);
  assert.deepEqual(opens().map((req) => req.fresh ?? false), [true]);
  assert.equal(terminal.scrollback, "");
  // Shown again (the view remounted), it reattaches and gets what the shell showed.
  const again = await openMainTerminal("new", tab, 80, 24);
  assert.deepEqual(opens().map((req) => req.fresh ?? false), [true, false]);
  assert.equal(again.id, terminal.id);
  assert.equal(again.scrollback, "startup\r\n");
  closeTab("new", tab);
});

test("after the daemon restarts, closing a terminal tab ends the shell it started for the tab", async () => {
  const tab = newSessionTab("restart", "terminal");
  const first = await openMainTerminal("restart", tab, 80, 24);
  running.clear(); // The daemon restarted: its shells are gone.
  const second = await openMainTerminal("restart", tab, 80, 24);
  assert.notEqual(second.id, first.id);
  closeTab("restart", tab);
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(sent.filter((req) => req.method === "closeTerminal").map((req) => req.terminalId), [second.id]);
  assert.equal(running.size, 0);
});

test("restarting a terminal tab whose shell exited starts a new shell", async () => {
  const tab = newSessionTab("exited", "terminal");
  const first = await openMainTerminal("exited", tab, 80, 24);
  running.delete(tab); // `exit`: the daemon lets the shell go.
  endMainTerminal(tab);
  const restarted = await openMainTerminal("exited", tab, 80, 24);
  assert.notEqual(restarted.id, first.id);
  assert.deepEqual(opens().map((req) => req.fresh ?? false), [true, true]);
  assert.equal(restarted.scrollback, "");
  closeTab("exited", tab);
});
