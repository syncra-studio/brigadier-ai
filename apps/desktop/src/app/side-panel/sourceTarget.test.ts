import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";

import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { Request, SourceFile, SourceState } from "@/ipc/generated";

import { mainAction, nothingStaged, sourceCalls, type SourceTarget } from "./sourceTarget";

const file = (status: SourceFile["status"]): SourceFile => ({ path: "a.ts", oldPath: null, status });
const state = (over: Partial<SourceState>): SourceState => ({
  branch: "main", remote: "origin", upstream: "origin/main", ahead: 0, behind: 0, staged: [], changes: [], merging: null, rebasing: null, ...over,
});
const merging = { incoming: "side", message: "Merge" };

test("the main button: Commit or Continue with something to commit, then Publish, then Sync, then a disabled Commit", () => {
  assert.deepEqual(mainAction(null, "ask"), { kind: "commit", enabled: false });
  // Something to commit comes first, whatever could be pushed.
  assert.deepEqual(mainAction(state({ staged: [file("modified")], ahead: 2 }), "never"), { kind: "commit", enabled: true });
  assert.deepEqual(mainAction(state({ changes: [file("modified")], upstream: null }), "ask"), { kind: "commit", enabled: true });
  assert.deepEqual(mainAction(state({ changes: [file("modified")] }), "always"), { kind: "commit", enabled: true });
  assert.deepEqual(mainAction(state({ changes: [file("modified")], ahead: 1 }), "never"), { kind: "commit", enabled: false }, "Never: off till something is staged");
  assert.deepEqual(mainAction(state({ merging }), "ask"), { kind: "continue", enabled: true });
  assert.deepEqual(mainAction(state({ merging, changes: [file("conflicted")] }), "ask"), { kind: "continue", enabled: false });
  // Nothing to commit.
  assert.deepEqual(mainAction(state({ upstream: null }), "ask"), { kind: "publish", enabled: true });
  assert.deepEqual(mainAction(state({ ahead: 2 }), "ask"), { kind: "sync", enabled: true });
  assert.deepEqual(mainAction(state({ behind: 1 }), "ask"), { kind: "sync", enabled: true });
  assert.deepEqual(mainAction(state({}), "ask"), { kind: "commit", enabled: false });
  // Nowhere to publish to, or no branch: nothing it could do.
  assert.deepEqual(mainAction(state({ upstream: null, remote: null }), "ask"), { kind: "commit", enabled: false });
  assert.deepEqual(mainAction(state({ branch: null, upstream: null }), "ask"), { kind: "commit", enabled: false });
});

test("Commit's stage-all is only for changes with nothing staged and nothing under way", () => {
  assert.equal(nothingStaged(state({ changes: [file("modified")] })), true);
  assert.equal(nothingStaged(state({ changes: [file("modified")], staged: [file("added")] })), false);
  assert.equal(nothingStaged(state({ changes: [file("modified")], merging })), false);
  assert.equal(nothingStaged(state({})), false);
});

const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
const asked: Request[] = [];
beforeEach(() => {
  asked.length = 0;
  Object.defineProperty(globalThis, "window", { configurable: true, value: {} });
  mockIPC((_command, args) => {
    const request = (args as { request: Request }).request;
    asked.push(request);
    return { method: request.method, state: state({}), outcome: {} };
  });
});
afterEach(() => {
  clearMocks();
  if (originalWindow) Object.defineProperty(globalThis, "window", originalWindow);
  else Reflect.deleteProperty(globalThis, "window");
});

test("each target's changes go to its own calls: a worktree's never to its project's folder", async () => {
  const worktree: SourceTarget = { type: "worktree", projectId: "p", path: "/w/one" };
  const stage = { type: "stage", paths: null } as const;
  await sourceCalls(worktree).change(stage);
  await sourceCalls(worktree).commit("m", false, false, true);
  await sourceCalls({ type: "project", projectId: "p" }).change(stage);
  await sourceCalls({ type: "project", projectId: "p" }).commit("m", false, false, false);
  await sourceCalls({ type: "conversation", conversationId: "c" }).change(stage);
  await sourceCalls({ type: "conversation", conversationId: "c" }).commit("m", false, false, true);
  assert.deepEqual(asked, [
    { method: "changeWorktreeSource", projectId: "p", path: "/w/one", change: stage },
    { method: "commitWorktreeChanges", projectId: "p", path: "/w/one", message: "m", includeUnstaged: true, push: false, amend: false },
    { method: "changeProjectSource", projectId: "p", change: stage },
    { method: "commitProjectChanges", projectId: "p", message: "m", includeUnstaged: false, push: false, amend: false },
    { method: "stageFiles", conversationId: "c", paths: null },
    { method: "commitChanges", conversationId: "c", message: "m", includeUnstaged: true, push: false, amend: false },
  ]);
});
