import assert from "node:assert/strict";
import { test } from "node:test";
import type { Conversation, Environment, Project } from "@/ipc/generated";
import { checkoutKey, remembered } from "./workspaceCache";

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
