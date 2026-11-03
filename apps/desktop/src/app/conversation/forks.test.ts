import assert from "node:assert/strict";
import { test } from "node:test";

import { folderSharers, forksUnderSources } from "@/app/conversation/forks";
import type { Conversation, Setup } from "@/ipc/generated";

const local = (branch: string): Setup => ({
  type: "session", repo: "/repo", environment: { type: "localCheckout", branch },
  permission: "full", orchestrator: { provider: "claude", model: "opus", effort: "high" }, workersSeeUncommitted: null, planMode: false,
} as Setup);
const session = (id: string, from: string | null = null, setup: Setup | null = null): Conversation => ({
  id, kind: "session", projectId: "p", title: id, pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup,
  lifecycle: "active", forkedFrom: from ? { conversationId: from, messageId: "m" } : null, sideOf: null, quotaWait: null,
} as Conversation);
const ids = (list: Conversation[]) => list.map((conversation) => conversation.id);

test("a fork sits right under its source, its own forks under it", () => {
  const list = [session("fork2", "a"), session("b"), session("a"), session("forkOfFork", "fork2"), session("fork1", "a")];
  assert.deepEqual(ids(forksUnderSources(list)), ["b", "a", "fork2", "forkOfFork", "fork1"]);
});

test("a fork whose source isn't listed keeps its place; a list with no forks is the same list", () => {
  const list = [session("x"), session("fork", "gone")];
  assert.deepEqual(ids(forksUnderSources(list)), ["x", "fork"]);
  assert.equal(forksUnderSources(list.slice(0, 1)).length, 1);
});

test("a same-folder fork and its source share the folder; a fork elsewhere doesn't", () => {
  const source = session("s", null, local("main"));
  const here = session("here", "s", local("s/fork"));
  const away = session("away", "s", { ...local("w"), environment: { type: "newWorktree", base: "main", branch: "w", path: "/wt", start: null } } as Setup);
  const all = { s: source, here, away };
  assert.deepEqual(ids(folderSharers(source, all)), ["here"]);
  assert.deepEqual(ids(folderSharers(here, all)), ["s"]);
  assert.deepEqual(folderSharers(away, all), []);
});
