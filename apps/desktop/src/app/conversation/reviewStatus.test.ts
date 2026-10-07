import assert from "node:assert/strict";
import { test } from "node:test";

import { mergeReviews, reviewStatus } from "@/app/conversation/reviewStatus";
import type { Approval, ReviewRun, ReviewState } from "@/ipc/generated";

const review = (id: string, startedAtMs: number, state: ReviewState, kind: ReviewRun["kind"] = "code"): ReviewRun => ({
  id,
  conversationId: "c1",
  requestId: null,
  taskId: null,
  kind,
  base: "a1",
  tip: `t-${id}`,
  author: "claude",
  reviewer: "codex",
  reviewerModel: "gpt-6.1-sol",
  notify: { type: "orchestrator" },
  state,
  startedAtMs,
  endedAtMs: null,
  findings: null,
});

const merge = (id: string, createdAtMs: number, state: Approval["state"]): Approval => ({
  id,
  conversationId: "c1",
  taskId: null,
  requestId: null,
  position: 0,
  subject: {
    type: "finishSession",
    branch: "brigadier/flow",
    base: "main",
    commits: 1,
    diffStat: { files: [], insertions: 0, deletions: 0 },
  },
  state,
  createdAtMs,
  resolvedAtMs: null,
});

test("a merge card says the review runs, then what it found, in plain words", () => {
  assert.equal(reviewStatus([]), null);
  assert.equal(reviewStatus([review("1", 1, { type: "running" })]), "Review running…");
  assert.equal(
    reviewStatus([review("1", 1, { type: "clean" }), review("2", 2, { type: "running" })]),
    "Review running…",
  );
  assert.equal(reviewStatus([review("1", 1, { type: "clean" })]), "Review: clean");
  assert.equal(
    reviewStatus([review("1", 1, { type: "findings", count: 1 }), review("2", 2, { type: "clean" })]),
    "Review: 1 finding",
  );
  assert.equal(
    reviewStatus([review("1", 1, { type: "findings", count: 2 }), review("2", 2, { type: "findings", count: 1 })]),
    "Review: 3 findings",
  );
  assert.equal(reviewStatus([review("1", 1, { type: "failed", reason: "no model" })]), "Review couldn’t run");
  assert.equal(
    reviewStatus([review("1", 1, { type: "failed", reason: "no model" }), review("2", 2, { type: "clean" })]),
    "Review: clean",
  );
});

test("a merge card speaks for the code reviews since the session's previous merge", () => {
  const earlier = { ...merge("m1", 10, { type: "allowed", by: "user", similar: false }), resolvedAtMs: 20 };
  const card = merge("m2", 40, { type: "pending" });
  const reviews = [
    review("old", 5, { type: "findings", count: 4 }),
    review("plan", 25, { type: "findings", count: 1 }, "plan"),
    review("new", 30, { type: "clean" }),
  ];
  assert.deepEqual(
    mergeReviews(card, [earlier, card], reviews).map((found) => found.id),
    ["new"],
  );
  // With no merge before it, every code review counts.
  assert.deepEqual(
    mergeReviews(card, [card], reviews).map((found) => found.id),
    ["old", "new"],
  );
});

test("a merged card keeps its own reviews and none of the work after it", () => {
  const card = { ...merge("m1", 10, { type: "allowed", by: "user", similar: false }), resolvedAtMs: 20 };
  const next = merge("m2", 60, { type: "pending" });
  const reviews = [
    // Still running at the merge: it stays the card's.
    review("landing", 8, { type: "running" }),
    review("later", 40, { type: "findings", count: 2 }),
  ];
  assert.deepEqual(
    mergeReviews(card, [card, next], reviews).map((found) => found.id),
    ["landing"],
  );
  assert.equal(reviewStatus(mergeReviews(card, [card, next], reviews)), "Review running…");
  assert.deepEqual(
    mergeReviews(next, [card, next], reviews).map((found) => found.id),
    ["later"],
  );
});

test("a merge card counts the reviews of what landed, not a worker's own review of its work", () => {
  const card = merge("m1", 40, { type: "pending" });
  const own = { ...review("own", 10, { type: "findings", count: 3 }), notify: { type: "worker" as const, taskId: "t1" } };
  const landing = review("landing", 30, { type: "clean" });
  assert.deepEqual(
    mergeReviews(card, [card], [own, landing]).map((found) => found.id),
    ["landing"],
  );
  assert.equal(reviewStatus(mergeReviews(card, [card], [own, landing])), "Review: clean");
});

test("a merge card counts the review of the thread's own commits with the landings'", () => {
  const card = merge("m1", 40, { type: "pending" });
  const landing = { ...review("landing", 20, { type: "clean" }), taskId: "t1" };
  const own = review("thread", 30, { type: "running" });
  assert.equal(own.taskId, null);
  assert.deepEqual(
    mergeReviews(card, [card], [landing, own]).map((found) => found.id),
    ["landing", "thread"],
  );
  assert.equal(reviewStatus(mergeReviews(card, [card], [landing, own])), "Review running…");
  const found = { ...own, state: { type: "findings", count: 2 } as const };
  assert.equal(reviewStatus(mergeReviews(card, [card], [landing, found])), "Review: 2 findings");
  const clean = { ...own, state: { type: "clean" } as const };
  assert.equal(reviewStatus(mergeReviews(card, [card], [landing, clean])), "Review: clean");
});
