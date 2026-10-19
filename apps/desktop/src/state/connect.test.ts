import assert from "node:assert/strict";
import { test } from "node:test";

import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import { connectAgent, useConnect } from "./connect";

test("a second click while asking to connect sends nothing more", async () => {
  const asked: string[] = [];
  const pending: Array<() => void> = [];
  const answer = () => pending.shift()?.();
  Object.defineProperty(globalThis, "window", { configurable: true, value: { crypto: globalThis.crypto } });
  mockIPC((command, payload) => {
    if (command !== "ipc_request") return null;
    const { request } = payload as { request: { method: string; provider: string } };
    asked.push(request.provider);
    return new Promise((resolve) => {
      pending.push(() => resolve({ method: request.method }));
    });
  });
  try {
    const first = connectAgent("claude");
    const second = connectAgent("claude");
    await second;
    assert.deepEqual(asked, ["claude"]);
    answer();
    await first;
    assert.equal(useConnect.getState().errors.claude, undefined);
    // Once answered, Connect can be asked again (after a failure, say).
    const again = connectAgent("claude");
    await new Promise((resolve) => setTimeout(resolve, 0));
    answer();
    await again;
    assert.deepEqual(asked, ["claude", "claude"]);
  } finally {
    clearMocks();
  }
});
