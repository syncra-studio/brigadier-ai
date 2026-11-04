import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import type { Conversation, Environment, Project } from "@/ipc/generated";
import { cachedWorkspace, checkoutKey, emitWorkspaceChanged, keepSource, onWorkspaceChanged, prefetchWorkspace, readFiles, readSource, readUserWorktrees, refreshSource, remembered, watchTargets, workspaceKey } from "./workspaceCache";

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
const source = { branch: "main", remote: null, upstream: null, ahead: 0, behind: 0, staged: [], changes: [], merging: null, rebasing: null };

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

for (const kind of ["files", "source"] as const) {
  test(`twenty changes during a slow ${kind} read share one follow-up that sees the latest change`, async () => {
    const replies: (() => void)[] = [];
    let latest = 0;
    mockIPC(() => {
      const seen = latest;
      return new Promise((resolve) => {
        replies.push(() => resolve(kind === "files" ? tree([`${seen}.ts`]) : { method: "getSourceState", state: { ...source, branch: `${seen}` } }));
      });
    });
    const read = kind === "files" ? readFiles : readSource;
    const before = read(target);
    const after = [];
    for (latest = 1; latest <= 20; latest++) {
      emitWorkspaceChanged(target);
      after.push(read(target));
    }
    latest = 20;
    assert.equal(replies.length, 1, "only the original read has started");
    // Drain every queued read so a failure leaves no requests hanging into another test.
    replies[0]!();
    await before;
    await new Promise((resolve) => setTimeout(resolve, 0));
    for (let index = 1; index < replies.length; index++) {
      replies[index]!();
      await new Promise((resolve) => setTimeout(resolve, 0));
    }
    const results = await Promise.all(after);
    assert.equal(replies.length, 2, "the burst queues just one follow-up");
    for (const result of results) {
      assert.deepEqual(result, kind === "files" ? { files: ["20.ts"], truncated: false, ignored: [] } : { ...source, branch: "20" });
    }
  });
}

const sourceReply = (branch: string) => ({ method: "getSourceState", state: { ...source, branch } });
const settled = () => new Promise((resolve) => setTimeout(resolve, 0));

test("opening a target with a cached source still reads it again", async () => {
  const asked: string[] = [];
  mockIPC((_, payload) => {
    const { method } = (payload as { request: { method: string } }).request;
    asked.push(method);
    return method === "getSourceState" ? sourceReply("fresh") : tree([]);
  });
  await readFiles(target);
  keepSource(target, source);
  asked.length = 0;
  prefetchWorkspace(target);
  await settled();
  assert.deepEqual(asked, ["getSourceState"]);
  assert.equal(cachedWorkspace(target)?.source?.branch, "fresh");
});

test("a read begun before a mutation can't publish over it, to the cache or to whoever holds its promise", async () => {
  const replies: ((value: unknown) => void)[] = [];
  mockIPC(() => new Promise((resolve) => { replies.push(resolve); }));
  const held = readSource(target);
  keepSource(target, { ...source, branch: "mutated" });
  replies[0]!(sourceReply("old"));
  await settled();
  assert.equal(cachedWorkspace(target)?.source?.branch, "mutated", "the old answer isn't kept");
  assert.equal(replies.length, 2, "it reads again, with no watcher event to ask for it");
  replies[1]!(sourceReply("newer"));
  assert.equal((await held).branch, "newer", "the holder never sees the old answer");
  assert.equal(cachedWorkspace(target)?.source?.branch, "newer");
});

test("a read that fails after a mutation began reads again, and one that fails alone still fails", async () => {
  const replies: { resolve: (value: unknown) => void; reject: (error: unknown) => void }[] = [];
  mockIPC(() => new Promise((resolve, reject) => { replies.push({ resolve, reject }); }));
  const held = readSource(target);
  keepSource(target, { ...source, branch: "mutated" });
  replies[0]!.reject(new Error("too early"));
  await settled();
  replies[1]!.resolve(sourceReply("newer"));
  assert.equal((await held).branch, "newer");
  const alone = readSource(target);
  replies[2]!.reject(new Error("no checkout"));
  await assert.rejects(alone, /no checkout/);
});

const worktreeTarget = { type: "worktree", projectId: "p", path: "/w/one" } as const;

test("each worktree has its own key, apart from its project's folder", () => {
  const projects = project("/a");
  const one = checkoutKey(worktreeTarget, {}, projects);
  assert.notEqual(one, checkoutKey({ type: "project", projectId: "p" }, {}, projects));
  assert.notEqual(one, checkoutKey({ ...worktreeTarget, path: "/w/two" }, {}, projects));
});

test("a worktree is read through its own call, and a read begun before a change is made again", async () => {
  const replies: ((value: unknown) => void)[] = [];
  const asked: unknown[] = [];
  mockIPC((_command, args) => {
    asked.push((args as { request: unknown }).request);
    return new Promise((resolve) => { replies.push(resolve); });
  });
  const reading = readSource(worktreeTarget);
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(asked, [{ method: "getWorktreeSourceState", projectId: "p", path: "/w/one" }]);
  // A stage or discard answers while it is on its way; nothing else says so.
  const changed = { ...source, branch: "after" };
  keepSource(worktreeTarget, changed);
  replies[0]!({ method: "getWorktreeSourceState", state: { ...source, branch: "before" } });
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(cachedWorkspace(worktreeTarget)?.source, changed, "the old read isn't kept");
  await new Promise((resolve) => setTimeout(resolve, 0));
  replies[1]!({ method: "getWorktreeSourceState", state: changed });
  assert.equal((await reading).branch, "after", "nor handed to whoever holds its promise");
  const refreshed = refreshSource(worktreeTarget);
  await new Promise((resolve) => setTimeout(resolve, 0));
  replies[2]!({ method: "getWorktreeSourceState", state: { ...source, branch: "refreshed" } });
  assert.equal((await refreshed).branch, "refreshed");
});

test("a change names its target: a worktree's listener hears its own and no other checkout's", () => {
  let heard = 0;
  const stop = onWorkspaceChanged(worktreeTarget, () => { heard++; });
  emitWorkspaceChanged({ type: "project", projectId: "p" });
  emitWorkspaceChanged({ ...worktreeTarget, path: "/w/two" });
  assert.equal(heard, 0);
  emitWorkspaceChanged(worktreeTarget);
  assert.equal(heard, 1);
  stop();
});

test("the watched set is what the sidebar shows: a draft's worktree rows only on Source", () => {
  const folder = { type: "project", projectId: "p" } as const;
  assert.deepEqual(watchTargets(null, ["/w/one"], true), [], "a closed sidebar watches nothing");
  assert.deepEqual(watchTargets(folder, undefined, true), [folder]);
  assert.deepEqual(watchTargets(folder, ["/w/one"], false), [folder]);
  assert.deepEqual(watchTargets(folder, ["/w/one"], true), [folder, worktreeTarget]);
  assert.deepEqual(watchTargets(target, ["/w/one"], true), [target]);
});

test("which worktrees the user made is read from the daemon", async () => {
  mockIPC(() => ({ method: "listUserWorktrees", paths: ["/w/one"] }));
  assert.deepEqual(await readUserWorktrees("p"), ["/w/one"]);
});
