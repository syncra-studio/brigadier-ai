import assert from "node:assert/strict";
import { test } from "node:test";

import { batchReviews, findingRows } from "@/app/conversation/reviewStatus";
import type { Finding, FindingStatus, ReviewRun, ReviewState } from "@/ipc/generated";

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
  number: 0,
  items: [],
});

const item = (id: string, status: FindingStatus): Finding =>
  ({ id, severity: "P2", title: `Finding ${id}`, location: "src/a.ts:1", status, high: false });
const found = (id: string, startedAtMs: number, items: Finding[]): ReviewRun =>
  ({ ...review(id, startedAtMs, { type: "findings", count: items.length }), items });

test("a click lists each finding with what the thread recorded", () => {
  const rows = findingRows([found("1", 1, [
    item("R1.1", { type: "fixed", task: "task-7", why: null, checked: "loader.rs:12 at the tip" }),
    item("R1.2", { type: "open", why: "needs the user" }),
    item("R1.3", { type: "dismissed", why: "the test covers it" }),
    item("R1.4", { type: "new" }),
    item("R1.5", { type: "fixed", task: "task-8", why: null, checked: null }),
  ])]);
  assert.deepEqual(rows.map((row) => [row.id, row.severity, row.status, row.note, row.open]), [
    ["R1.1", "P2", "Fixed", "Fixed in task-7. Checked: loader.rs:12 at the tip", false],
    ["R1.2", "P2", "Open", "needs the user", true],
    ["R1.3", "P2", "Dismissed", "the test covers it", false],
    ["R1.4", "P2", "New", null, false],
    ["R1.5", "P2", "Unverified", "Fixed in task-8. Not checked against the merged code", false],
  ]);
});

test("a batch holds the reviews of what landed and of the thread's own commits, not a worker's own review or a plan's", () => {
  const own = { ...review("1", 10, { type: "findings", count: 3 }), notify: { type: "worker", taskId: "t1" } } as ReviewRun;
  const landing = review("2", 20, { type: "clean" });
  const thread = review("3", 30, { type: "running" });
  const plan = review("4", 35, { type: "findings", count: 1 }, "plan");
  assert.deepEqual(
    batchReviews([own, landing, thread, plan], 0, 40).map((batch) => batch.id),
    ["2", "3"],
  );
});
