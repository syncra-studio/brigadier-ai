import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import { account } from "@/fixtures/agents";
import type { AccountView, EventEnvelope, Request } from "@/ipc/generated";
import { applyEvents, useApp } from "@/state/store";
import { checkedFor, loadUsage, refreshUsage, useUsage } from "@/state/usage";

let calls: string[];
const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window");

beforeEach(() => {
  Object.defineProperty(globalThis, "window", { configurable: true, value: {} });
  calls = [];
  useApp.setState({ accounts: { accounts: accounts(0), warnings: [] } });
  useUsage.setState({ refreshing: false, refreshError: null, view: null, activityShown: false });
  mockIPC((command, payload) => {
    if (command !== "ipc_request") return null;
    const req = (payload as { request: Request }).request;
    calls.push(req.method);
    if (req.method === "refreshAccounts") return { method: req.method, generation: 3 };
    if (req.method === "getUsage") return { method: req.method, usage: { providers: [] } };
    return { method: req.method };
  });
});
afterEach(() => {
  clearMocks();
  if (originalWindow) Object.defineProperty(globalThis, "window", originalWindow);
  else Reflect.deleteProperty(globalThis, "window");
});

function accounts(checkedGeneration: number, checking = false): AccountView[] {
  return [
    account("claude", undefined, "me@example.com", { checkedGeneration, checking }),
    account("claude", "acct-work", "work@example.com", { checkedGeneration, checking }),
  ];
}
function checked(views: AccountView[]) {
  applyEvents([
    { seq: 1, stream: "accounts", streamSeq: 1, atMs: 1, event: { type: "accountsChecked", accounts: { accounts: views } } } as EventEnvelope,
  ]);
}
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

test("usage reads all projects without a project filter", async () => {
  const requests: Request[] = [];
  mockIPC((command, payload) => {
    if (command !== "ipc_request") return null;
    const req = (payload as { request: Request }).request;
    requests.push(req);
    return { method: req.method, usage: { providers: [] } };
  });
  await loadUsage();
  assert.deepEqual(requests, [{ method: "getUsage" }]);
  assert.deepEqual(useUsage.getState().view, { providers: [] });
});

test("an account counts as checked for a refresh once a check that began in it finished", () => {
  assert.equal(checkedFor(accounts(2), 3), false);
  assert.equal(checkedFor(accounts(3, true), 3), false, "a check still running doesn't count");
  assert.equal(checkedFor(accounts(3), 3), true);
  assert.equal(checkedFor(accounts(4), 3), true);
});

test("Refresh checks every account again and reads usage once they were all checked for it", async () => {
  const done = refreshUsage();
  await tick();
  assert.deepEqual(calls, ["refreshAccounts"]);
  assert.equal(useUsage.getState().refreshing, true);
  // A check that began before this refresh finishes: still waiting.
  checked(accounts(2));
  await tick();
  assert.equal(useUsage.getState().refreshing, true);
  checked([accounts(3)[0]!, accounts(2)[1]!]);
  await tick();
  assert.equal(useUsage.getState().refreshing, true, "every account, not just one");
  checked(accounts(3));
  await done;
  assert.deepEqual(calls, ["refreshAccounts", "getUsage"]);
  assert.equal(useUsage.getState().refreshing, false);
  assert.equal(useUsage.getState().refreshError, null);
});

test("Refresh says so when an account doesn't answer in time", async () => {
  await refreshUsage(20);
  assert.deepEqual(calls, ["refreshAccounts", "getUsage"]);
  assert.equal(useUsage.getState().refreshing, false);
  assert.match(useUsage.getState().refreshError ?? "", /Not every account answered in time/);
});
