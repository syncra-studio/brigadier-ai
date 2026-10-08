import assert from "node:assert/strict";
import { test } from "node:test";

import { batchReviews, reviewLines, reviewStatus } from "@/app/conversation/reviewStatus";
import type { OrchestratorStep, ReviewRun, ReviewState } from "@/ipc/generated";

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

const merged = (atMs: number): OrchestratorStep => ({
  requestId: null,
  kind: { type: "merged", branch: "brigadier/flow", base: "main", commits: 1, askedIn: "m1" },
  atMs,
  position: 0,
});

test("the review line says the review runs, then what it found, in plain words", () => {
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

test("before any merge, the context card's line speaks for all the session's code reviews", () => {
  const reviews = [review("1", 10, { type: "clean" }), review("2", 20, { type: "findings", count: 2 })];
  assert.deepEqual(reviewLines([], reviews), { merged: null, current: "Review: 2 findings" });
  assert.deepEqual(reviewLines([], []), { merged: null, current: null });
});

test("a review still running at the merge stays the merged work's, and so does its outcome", () => {
  const running = review("1", 10, { type: "running" });
  assert.deepEqual(reviewLines([merged(20)], [running]), { merged: "Review running…", current: null });
  const found = { ...running, state: { type: "findings", count: 1 } } as ReviewRun;
  assert.deepEqual(reviewLines([merged(20)], [found]), { merged: "Review: 1 finding", current: null });
  // New work after the merge has its own line; the merged work's stays until the next merge.
  const after = review("2", 30, { type: "running" });
  assert.deepEqual(reviewLines([merged(20)], [found, after]), {
    merged: "Review: 1 finding",
    current: "Review running…",
  });
  const done = { ...after, state: { type: "clean" } } as ReviewRun;
  assert.deepEqual(reviewLines([merged(20), merged(40)], [found, done]), {
    merged: "Review: clean",
    current: null,
  });
});

test("the lines count the reviews of what landed and of the thread's own commits, not a worker's own review", () => {
  const own = { ...review("1", 10, { type: "findings", count: 3 }), notify: { type: "worker", taskId: "t1" } } as ReviewRun;
  const landing = review("2", 20, { type: "clean" });
  const thread = review("3", 30, { type: "running" });
  const plan = review("4", 35, { type: "findings", count: 1 }, "plan");
  assert.deepEqual(
    batchReviews([own, landing, thread, plan], 0, 40).map((found) => found.id),
    ["2", "3"],
  );
  assert.deepEqual(reviewLines([merged(40)], [own, landing, thread, plan]), {
    merged: "Review running…",
    current: null,
  });
});
