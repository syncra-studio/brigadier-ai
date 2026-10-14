import assert from "node:assert/strict";
import { test } from "node:test";

import { batchReviews, checksLine, findingRows, reviewLines, reviewRow, reviewStatus } from "@/app/conversation/reviewStatus";
import type { Finding, FindingStatus, IntegratedCheck, IntegratedCheckState, OrchestratorStep, ReviewRun, ReviewState } from "@/ipc/generated";

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

const merged = (atMs: number): OrchestratorStep => ({
  requestId: null,
  kind: { type: "merged", branch: "brigadier/flow", base: "main", commits: 1, askedIn: "m1" },
  atMs,
  position: 0,
});

const item = (id: string, status: FindingStatus): Finding =>
  ({ id, severity: "P2", title: `Finding ${id}`, location: "src/a.ts:1", status, high: false });
const found = (id: string, startedAtMs: number, items: Finding[]): ReviewRun =>
  ({ ...review(id, startedAtMs, { type: "findings", count: items.length }), items });
const open = (id: string) => item(id, { type: "open", why: null });
const fixed = (id: string) => item(id, { type: "fixed", task: null, why: null });
const text = (lines: ReturnType<typeof reviewLines>) => ({ merged: lines.merged?.text ?? null, current: lines.current?.text ?? null });

test("the review line says the review runs, then what the thread recorded, in plain words", () => {
  assert.equal(reviewStatus([]), null);
  assert.equal(reviewStatus([review("1", 1, { type: "running" })]), "Review running…");
  assert.equal(
    reviewStatus([review("1", 1, { type: "clean" }), review("2", 2, { type: "running" })]),
    "Review running…",
  );
  assert.equal(reviewStatus([review("1", 1, { type: "clean" })]), "Review: clean");
  assert.equal(reviewStatus([review("1", 1, { type: "failed", reason: "no model" })]), "Review couldn’t run");
  assert.equal(
    reviewStatus([review("1", 1, { type: "failed", reason: "no model" }), review("2", 2, { type: "clean" })]),
    "Review: clean",
  );
});

test("two reviews, 3 + 2 findings, 4 fixed and 1 dismissed, read clean; one left open reads 1 open", () => {
  const fixedIn = (id: string): Finding => item(id, { type: "fixed", task: "task-7", why: null });
  const first = found("1", 1, [fixedIn("R1.1"), fixedIn("R1.2"), fixedIn("R1.3")]);
  const second = found("2", 2, [fixedIn("R2.1"), item("R2.2", { type: "dismissed", why: "by design" })]);
  assert.equal(reviewStatus([first, second]), "Review: clean");
  const leftOpen = found("2", 2, [fixedIn("R2.1"), item("R2.2", { type: "open", why: null })]);
  assert.equal(reviewStatus([first, leftOpen]), "Review: 1 open");
  // Not recorded yet: "new", never the reviews' raw count.
  const unrecorded = found("2", 2, [item("R2.1", { type: "new" }), item("R2.2", { type: "open", why: null })]);
  assert.equal(reviewStatus([first, unrecorded]), "Review: 1 open · 1 new");
  // A review from before findings were items: its count was never recorded.
  assert.equal(reviewStatus([review("3", 3, { type: "findings", count: 2 })]), "Review: 2 new");
});

test("a click lists each finding with what the thread recorded", () => {
  const rows = findingRows([found("1", 1, [
    item("R1.1", { type: "fixed", task: "task-7", why: null }),
    item("R1.2", { type: "open", why: "needs the user" }),
    item("R1.3", { type: "dismissed", why: "the test covers it" }),
    item("R1.4", { type: "new" }),
  ])]);
  assert.deepEqual(rows.map((row) => [row.id, row.severity, row.status, row.note, row.open]), [
    ["R1.1", "P2", "Fixed", "Fixed in task-7", false],
    ["R1.2", "P2", "Open", "needs the user", true],
    ["R1.3", "P2", "Dismissed", "the test covers it", false],
    ["R1.4", "P2", "New", null, false],
  ]);
});

test("after a merge, the line counts only that merge's open findings, never across merges", () => {
  const first = found("1", 10, [open("R1.1"), open("R1.2"), fixed("R1.3")]);
  const second = found("2", 30, [open("R2.1"), fixed("R2.2")]);
  assert.deepEqual(text(reviewLines([merged(20)], [first])), { merged: "Merged · 2 open (not fixed)", current: null });
  assert.deepEqual(text(reviewLines([merged(20)], [first, second])), {
    merged: "Merged · 2 open (not fixed)",
    current: "Review: 1 open",
  });
  // The second merge's line is its own work's: 1, not 3.
  assert.deepEqual(text(reviewLines([merged(20), merged(40)], [first, second])), {
    merged: "Merged · 1 open (not fixed)",
    current: null,
  });
  const clean = found("3", 50, [fixed("R3.1")]);
  assert.deepEqual(text(reviewLines([merged(20), merged(40), merged(60)], [first, second, clean])), {
    merged: "Merged · review clean",
    current: null,
  });
  assert.equal(reviewLines([merged(20)], [first]).merged?.findings.length, 3);
});

test("before any merge, the context card's line speaks for all the session's code reviews", () => {
  const reviews = [review("1", 10, { type: "clean" }), found("2", 20, [item("R2.1", { type: "open", why: null })])];
  assert.deepEqual(text(reviewLines([], reviews)), { merged: null, current: "Review: 1 open" });
  assert.deepEqual(reviewLines([], []), { merged: null, current: null });
});

test("a review still running at the merge stays the merged work's, and so does its outcome", () => {
  const running = review("1", 10, { type: "running" });
  assert.deepEqual(text(reviewLines([merged(20)], [running])), { merged: "Merged · review running…", current: null });
  const ended = found("1", 10, [item("R1.1", { type: "new" })]);
  assert.deepEqual(text(reviewLines([merged(20)], [ended])), { merged: "Merged · 1 new", current: null });
  // New work after the merge has its own line; the merged work's stays until the next merge.
  const after = review("2", 30, { type: "running" });
  assert.deepEqual(text(reviewLines([merged(20)], [ended, after])), {
    merged: "Merged · 1 new",
    current: "Review running…",
  });
  const done = { ...after, state: { type: "clean" } } as ReviewRun;
  assert.deepEqual(text(reviewLines([merged(20), merged(40)], [ended, done])), {
    merged: "Merged · review clean",
    current: null,
  });
});

test("the lines count the reviews of what landed and of the thread's own commits, not a worker's own review", () => {
  const own = { ...review("1", 10, { type: "findings", count: 3 }), notify: { type: "worker", taskId: "t1" } } as ReviewRun;
  const landing = review("2", 20, { type: "clean" });
  const thread = review("3", 30, { type: "running" });
  const plan = review("4", 35, { type: "findings", count: 1 }, "plan");
  assert.deepEqual(
    batchReviews([own, landing, thread, plan], 0, 40).map((batch) => batch.id),
    ["2", "3"],
  );
  assert.deepEqual(text(reviewLines([merged(40)], [own, landing, thread, plan])), {
    merged: "Merged · review running…",
    current: null,
  });
});

test("merged work and work since, both clean, read as one row: Review: clean", () => {
  const before = found("1", 10, [fixed("R1.1")]);
  const after = review("2", 30, { type: "clean" });
  const row = reviewRow(reviewLines([merged(20)], [before, after]));
  assert.equal(row?.text, "Review: clean");
  // The list still keeps each batch apart, with what the thread recorded of each.
  assert.deepEqual(row?.sections.map((section) => [section.label, section.outcome, section.findings.length]), [
    ["Merged work", "clean", 1],
    ["Since the merge", "clean", 0],
  ]);
  assert.equal(reviewRow(reviewLines([], [])), null);
});

test("a newer review never hides what the merge left open, and the counts stay apart", () => {
  const before = found("1", 10, [open("R1.1"), fixed("R1.2")]);
  const running = review("2", 30, { type: "running" });
  const row = reviewRow(reviewLines([merged(20)], [before, running]));
  assert.equal(row?.text, "Merged: 1 open · Since the merge: running…");
  assert.deepEqual(row?.sections.map((section) => [section.label, section.outcome]), [
    ["Merged work", "1 open"],
    ["Since the merge", "running…"],
  ]);
  // Two open findings, one in each batch: 1 and 1, never 2.
  const after = found("3", 30, [open("R3.1")]);
  assert.equal(reviewRow(reviewLines([merged(20)], [before, after]))?.text, "Merged: 1 open · Since the merge: 1 open");
});

test("one batch alone keeps its own words in an unlabelled list", () => {
  const before = found("1", 10, [open("R1.1")]);
  assert.deepEqual(reviewRow(reviewLines([merged(20)], [before])), {
    text: "Merged · 1 open (not fixed)",
    sections: [{ label: null, outcome: "1 open", findings: findingRows([before]) }],
  });
  const now = review("1", 10, { type: "running" });
  assert.equal(reviewRow(reviewLines([], [now]))?.text, "Review running…");
});

const check = (id: string, state: IntegratedCheckState): IntegratedCheck => ({
  id,
  conversationId: "c1",
  repo: "/repo",
  branch: "work",
  tip: "abcdef1234567890",
  state,
  dueAtMs: 0,
  startedAtMs: null,
  endedAtMs: null,
  requests: [],
  tasks: [],
  rerun: false,
});

test("the checks line follows the newest run of the session's full checks", () => {
  assert.equal(checksLine([]), null);
  const passed = check("check-9", { type: "passed", commands: ["tools/full-checks.sh"] });
  assert.deepEqual(checksLine([passed]), {
    text: "Checks passed",
    title: "tools/full-checks.sh at abcdef1234",
    failed: false,
  });
  // Numbered, not ordered as strings: check-10 is newer than check-9.
  const running = check("check-10", { type: "running", command: "tools/full-checks.sh" });
  assert.equal(checksLine([running, passed])?.text, "Checks running…");
  assert.equal(checksLine([passed, check("check-10", { type: "due" })])?.text, "Checks running…");
  // While they run, the row says for how long: since they started, else since they were due.
  assert.equal(checksLine([{ ...running, startedAtMs: 5_000 }])?.sinceMs, 5_000);
  assert.equal(checksLine([{ ...running, dueAtMs: 2_000 }])?.sinceMs, 2_000);
  const failed = check("check-11", {
    type: "failed",
    command: "cargo test --workspace",
    status: "exit 101",
    output: "out-3",
  });
  assert.deepEqual(checksLine([passed, running, failed]), {
    text: "Checks failed: cargo test --workspace",
    title: "exit 101 at abcdef1234",
    failed: true,
  });
  const cut = check("check-12", { type: "interrupted", reason: "a restart" });
  assert.deepEqual(checksLine([failed, cut]), { text: "Checks couldn't run", title: "a restart", failed: true });
});
