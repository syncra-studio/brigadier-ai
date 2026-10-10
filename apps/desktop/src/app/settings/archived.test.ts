import assert from "node:assert/strict";
import { test } from "node:test";

import {
  ALL_ARCHIVED,
  archivedGroups,
  archivedOf,
  groupCount,
  narrowed,
  projectChoices,
  shownIds,
} from "@/app/settings/archived";
import type { Conversation, Project } from "@/ipc/generated";

function conversation(id: string, fields: Partial<Conversation>): Conversation {
  return {
    id,
    kind: "chat",
    projectId: null,
    title: id,
    pinnedAtMs: null,
    createdAtMs: 0,
    updatedAtMs: 0,
    setup: null,
    lifecycle: "archived",
    forkedFrom: null,
    sideOf: null,
    fallback: null,
    quotaWait: null,
    ...fields,
  };
}

const projects = {
  p1: { id: "p1", name: "Zeta" },
  p2: { id: "p2", name: "alpha" },
} as unknown as Record<string, Project>;

const all = {
  a: conversation("a", { kind: "session", projectId: "p1", title: "Fix the login bug", createdAtMs: 1, updatedAtMs: 50 }),
  b: conversation("b", { kind: "session", projectId: "p2", title: "Add billing", createdAtMs: 3, updatedAtMs: 40 }),
  c: conversation("c", { title: "Trip plan", createdAtMs: 5, updatedAtMs: 60 }),
  d: conversation("d", { kind: "session", projectId: "p1", title: "billing export", createdAtMs: 4, updatedAtMs: 10 }),
  e: conversation("e", { title: "Live chat", lifecycle: "active", updatedAtMs: 99 }),
};

const archived = archivedOf(all);

test("only archived conversations show, newest first", () => {
  assert.deepEqual(
    archived.map((c) => c.id),
    ["c", "a", "b", "d"],
  );
});

test("groups are projects in order of their newest row, chats last", () => {
  const groups = archivedGroups(archived, projects, ALL_ARCHIVED);
  assert.deepEqual(
    groups.map((g) => [g.label, g.conversations.map((c) => c.id)]),
    [
      ["Zeta", ["a", "d"]],
      ["alpha", ["b"]],
      ["Chats", ["c"]],
    ],
  );
  assert.deepEqual(shownIds(groups), ["a", "d", "b", "c"]);
  assert.equal(groupCount(groups[0]!), "2 sessions");
  assert.equal(groupCount(groups[2]!), "1 chat");
});

test("sorting by created or by name reorders rows and groups", () => {
  const created = archivedGroups(archived, projects, { ...ALL_ARCHIVED, sort: "created" });
  assert.deepEqual(shownIds(created), ["d", "a", "b", "c"]);
  const named = archivedGroups(archived, projects, { ...ALL_ARCHIVED, sort: "alphabetical" });
  assert.deepEqual(shownIds(named), ["b", "d", "a", "c"]);
  assert.deepEqual(
    named.map((g) => g.label),
    ["alpha", "Zeta", "Chats"],
  );
});

const ids = (filter: Partial<typeof ALL_ARCHIVED>) =>
  shownIds(archivedGroups(archived, projects, { ...ALL_ARCHIVED, ...filter }));

test("search, kind and project narrow the rows", () => {
  assert.deepEqual(ids({ query: "BILLING" }), ["b", "d"]);
  assert.deepEqual(ids({ query: "billing exp" }), ["d"]);
  assert.deepEqual(ids({ kind: "chat" }), ["c"]);
  assert.deepEqual(ids({ kind: "session" }), ["a", "d", "b"]);
  assert.deepEqual(ids({ project: "p1" }), ["a", "d"]);
  assert.deepEqual(ids({ project: "chats" }), ["c"]);
  assert.deepEqual(ids({ project: "p2", kind: "chat" }), []);
  assert.equal(narrowed(ALL_ARCHIVED), false);
  assert.equal(narrowed({ ...ALL_ARCHIVED, sort: "created" }), false);
  assert.equal(narrowed({ ...ALL_ARCHIVED, query: "x" }), true);
});

test("the project menu lists projects with archived rows by name, then the chats", () => {
  assert.deepEqual(
    projectChoices(archived, projects).map((choice) => choice.label),
    ["All projects", "alpha", "Zeta", "Chats"],
  );
  assert.deepEqual(
    projectChoices(archived.filter((c) => c.kind === "session"), projects).map((choice) => choice.value),
    ["all", "p2", "p1"],
  );
});
