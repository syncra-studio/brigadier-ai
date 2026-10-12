import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import type { Conversation, Environment, Project } from "@/ipc/generated";
import { cachedWorkspace, checkoutKey, emitWorkspaceChanged, keepSource, readFiles, remembered, workspaceKey } from "./workspaceCache";

import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { useApp } from "@/state/store";

const session = (environment: Environment, lifecycle: Conversation["lifecycle"] = "active") => ({
  s: { id: "s", kind: "session", lifecycle, setup: { type: "session", repo: "/repo", environment } } as unknown as Conversation,
});
const worktree = (path: string | null): Environment => ({ type: "newWorktree", base: "main", branch: "b", path, start: null });
const key = (conversations: Record<string, Conversation>) => checkoutKey({ type: "conversation", conversationId: "s" }, conversations, {});

test("a session reads another checkout, so another key, when it moves to its own copy or loses and gets it back", () => {
  const beforeCopy = key(session(worktree(null)));
  const inCopy = key(session(worktree("/copies/s")));
  const archived = key(session(worktree("/copies/s"), "archived"));
  assert.equal(new Set([beforeCopy, inCopy, archived]).size, 3);
  assert.equal(key(session(worktree("/copies/s"))), inCopy, "the same checkout keeps its key");
  assert.notEqual(key(session({ type: "localCheckout", branch: "main" })), inCopy);
});

const project = (path: string) => ({ p: { id: "p", repos: [{ path, name: "r" }] } as unknown as Project });

test("a draft's key follows its project's folders", () => {
  const draft = { type: "project", projectId: "p" } as const;
  assert.notEqual(checkoutKey(draft, {}, project("/a")), checkoutKey(draft, {}, project("/b")));
});

test("the cache keeps the few checkouts used last, and a write merges into its entry", () => {
  let state: Parameters<typeof remembered>[0] = { entries: {}, order: [] };
  for (const name of ["a", "b", "c"]) state = remembered(state, name, { files: { files: [name], truncated: false } }, 2);
  assert.deepEqual(state.order, ["b", "c"]);
  assert.deepEqual(Object.keys(state.entries).toSorted(), ["b", "c"]);
  state = remembered(state, "b", { source: { branch: "main" } as never }, 2);
  assert.deepEqual(state.order, ["c", "b"], "a write makes it the newest");
  assert.deepEqual(state.entries.b, { files: { files: ["b"], truncated: false }, source: { branch: "main" } });
});


test("the cache evicts by bytes as well as entry count, including an oversized response", () => {
  const patch = { files: { files: ["a".repeat(200)], truncated: false } };
  let state = remembered({ entries: {}, order: [] }, "a", patch, 4, 700);
  state = remembered(state, "b", patch, 4, 700);
  assert.deepEqual(state.order, ["b"]);
  assert.ok(JSON.stringify(state.entries).length * 2 < 700);
  assert.deepEqual(remembered(state, "huge", patch, 4, 100), { entries: {}, order: [] });
});

const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
beforeEach(() => {
  Object.defineProperty(globalThis, "window", { configurable: true, value: {} });
  useApp.setState({ conversations: session(worktree("/copies/s")), projects: {} });
});
afterEach(() => {
  clearMocks();
  if (originalWindow) Object.defineProperty(globalThis, "window", originalWindow);
  else Reflect.deleteProperty(globalThis, "window");
});
const target = { type: "conversation", conversationId: "s" } as const;
const source = { branch: "main", remote: null, upstream: null, ahead: 0, staged: [], changes: [] };

test("archive and restore at the same path discard the earlier generation; ordinary updates keep it", () => {
  const initial = workspaceKey(target);
  keepSource(target, source);
  useApp.setState({ conversations: { s: { ...useApp.getState().conversations.s!, title: "Renamed" } } });
  assert.equal(workspaceKey(target), initial);
  assert.deepEqual(cachedWorkspace(target)?.source, source);
  useApp.setState({ conversations: session(worktree("/copies/s"), "archived") });
  useApp.setState({ conversations: session(worktree("/copies/s")) });
  assert.notEqual(workspaceKey(target), initial);
  assert.equal(cachedWorkspace(target), undefined);
  keepSource(target, source, initial);
  assert.equal(cachedWorkspace(target), undefined, "a mutation from the old view cannot populate the new checkout");
});

test("a late read from a removed checkout cannot be reused, even after the same path returns", async () => {
  const replies: ((value: unknown) => void)[] = [];
  mockIPC(() => new Promise((resolve) => { replies.push(resolve); }));
  const first = readFiles(target);
  const shared = readFiles(target);
  assert.equal(replies.length, 1, "prefetch and open share a read");
  useApp.setState({ conversations: session(worktree(null)) });
  useApp.setState({ conversations: session(worktree("/copies/s")) });
  const current = readFiles(target);
  assert.equal(replies.length, 2, "a new generation never shares the earlier request");
  replies[1]!({ method: "getWorkspaceTree", tree: { files: ["current.ts"], truncated: false, ignored: [], ignoredTruncated: false } });
  await current;
  replies[0]!({ method: "getWorkspaceTree", tree: { files: ["old.ts"], truncated: false, ignored: [], ignoredTruncated: false } });
  await Promise.all([first, shared]);
  assert.deepEqual(cachedWorkspace(target)?.files?.files, ["current.ts"]);
});

const tree = (files: string[]) => ({ method: "getWorkspaceTree", tree: { files, truncated: false, ignored: [], ignoredTruncated: false } });

test("a read begun before a change isn't shared after it: one more follows it, never alongside", async () => {
  const replies: ((value: unknown) => void)[] = [];
  mockIPC(() => new Promise((resolve) => { replies.push(resolve); }));
  const before = readFiles(target);
  emitWorkspaceChanged(target);
  const after = readFiles(target);
  const alsoAfter = readFiles(target);
  assert.equal(replies.length, 1, "the next read waits for the one under way");
  replies[0]!(tree(["before.ts"]));
  assert.deepEqual((await before).files, ["before.ts"]);
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(replies.length, 2, "then reads again, once for both");
  replies[1]!(tree(["after.ts"]));
  assert.deepEqual((await after).files, ["after.ts"]);
  assert.deepEqual((await alsoAfter).files, ["after.ts"]);
  assert.deepEqual(cachedWorkspace(target)?.files?.files, ["after.ts"]);
});

test("deleting and recreating a project or changing a session repo gives a new generation", () => {
  const draft = { type: "project", projectId: "p" } as const;
  useApp.setState({ projects: project("/a") });
  const initial = workspaceKey(draft);
  keepSource(draft, source);
  useApp.setState({ projects: {} });
  useApp.setState({ projects: project("/a") });
  assert.notEqual(workspaceKey(draft), initial);
  assert.equal(cachedWorkspace(draft), undefined);
  const old = workspaceKey(target);
  const conversation = useApp.getState().conversations.s!;
  useApp.setState({ conversations: { s: { ...conversation, setup: { ...conversation.setup!, repo: "/other" } as Conversation["setup"] } } });
  assert.notEqual(workspaceKey(target), old);
});
