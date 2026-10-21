import assert from "node:assert/strict";
import { test } from "node:test";

import { type JobInput, sessionJobs } from "@/app/conversation/boardJob";
import { jobLabel } from "@/app/conversation/job";
import type { IntegratedCheck, IntegratedCheckState, OrchestratorStep, OvernightRun, Plan, ReviewRun, ReviewState, Task, TaskState, UserRequest } from "@/ipc/generated";

const task = (id: string, state: TaskState, createdAtMs: number, extra: Partial<Task> = {}): Task =>
  ({ id, number: 1, title: `Task ${id}`, kind: "implement", state, requestId: "r1", subject: null, createdAtMs, updatedAtMs: createdAtMs + 5, ...extra }) as unknown as Task;

const review = (id: string, startedAtMs: number, state: ReviewState, items: ReviewRun["items"] = []): ReviewRun =>
  ({ id, conversationId: "c", requestId: "r1", taskId: null, kind: "code", notify: { type: "orchestrator" }, state, startedAtMs, endedAtMs: state.type === "running" ? null : startedAtMs + 3, number: 1, items }) as unknown as ReviewRun;

const check = (n: number, dueAtMs: number, state: IntegratedCheckState): IntegratedCheck =>
  ({ id: `check-${n}`, conversationId: "c", repo: "/r", branch: "b", tip: "abcdef1234", state, dueAtMs, startedAtMs: dueAtMs + 1, endedAtMs: state.type === "running" || state.type === "due" ? null : dueAtMs + 60_001, requests: ["r1"], tasks: [], rerun: false });

const step = (atMs: number, kind: OrchestratorStep["kind"]): OrchestratorStep => ({ requestId: "r1", kind, atMs, position: 0 });

const input = (parts: Partial<JobInput>): JobInput => ({
  tasks: {}, reviews: {}, integratedChecks: {}, orchestratorSteps: [], plans: {}, requests: {}, overnight: {},
  reviewWork: true, autoMerge: true, ...parts,
});
const byId = <T extends { id: string }>(items: T[]) => Object.fromEntries(items.map((item) => [item.id, item]));
const steps = (parts: Partial<JobInput>) => sessionJobs(input(parts)).current?.steps.map((s) => `${s.id}:${s.state}`);

test("a question has no job; a worker at work is Building, with the review, checks and merge to come", () => {
  assert.equal(sessionJobs(input({})).current, null);
  const parts = { tasks: byId([task("t1", "running", 10)]) };
  assert.deepEqual(steps(parts), ["build:active", "review:pending", "checks:pending", "merge:pending"]);
  assert.equal(jobLabel(sessionJobs(input(parts)).current!), "Building");
  // Off, no review and no merge of its own are to come.
  assert.deepEqual(steps({ ...parts, reviewWork: false, autoMerge: false }), ["build:active", "checks:pending"]);
});

test("a review at work counts its reviews; a failure reads first, then the fix at work", () => {
  const reviews = byId([review("v1", 20, { type: "clean" }), review("v2", 21, { type: "running" })]);
  const job = sessionJobs(input({ tasks: byId([task("t1", "landed", 10)]), reviews })).current!;
  assert.deepEqual(job.steps.map((s) => `${s.id}:${s.state}`), ["build:done", "review:active", "checks:pending", "merge:pending"]);
  assert.equal(jobLabel(job), "Reviewing · 1 of 2");

  const failed = sessionJobs(input({
    tasks: byId([task("t1", "landed", 10), task("t2", "running", 40)]),
    reviews: byId([review("v1", 20, { type: "findings", count: 1 }, [{ id: "R1.1", severity: "P2", title: "A bug", location: null, status: { type: "fixed", task: "task-2", why: null, checked: "read" }, high: false }])]),
    integratedChecks: byId([check(1, 30, { type: "failed", command: "pnpm test", status: "exit 1", output: null })]),
  })).current!;
  assert.deepEqual(failed.steps.map((s) => `${s.id}:${s.state}`), ["build:done", "review:done", "fix:active", "checks:failed", "merge:pending"]);
  assert.equal(jobLabel(failed), "Checks failed · fixing");
  assert.equal(failed.steps.find((s) => s.id === "review")?.summary, "Review found 1 issue");
  assert.equal(failed.steps.find((s) => s.id === "checks")?.summary, "pnpm test: exit 1");
});

test("a change all done, nothing at work, has no job; idle with a failure, the failure stays", () => {
  const done = { tasks: byId([task("t1", "landed", 10)]), reviews: byId([review("v1", 20, { type: "clean" })]), integratedChecks: byId([check(1, 30, { type: "passed", commands: ["pnpm test"] })]) };
  assert.equal(sessionJobs(input(done)).current, null);
  const failed = { ...done, integratedChecks: byId([check(1, 30, { type: "failed", command: "pnpm test", status: "exit 1", output: null })]) };
  assert.deepEqual(steps(failed), ["build:done", "review:done", "fix:pending", "checks:failed"]);
  assert.equal(jobLabel(sessionJobs(input(failed)).current!), "Checks failed");
});

test("a merge closes the job: the tab keeps it, with its findings and record", () => {
  const record = { head: "abc", checks: [], reviews: [], ci: null, proofs: 0, verifier: null };
  const parts = {
    tasks: byId([task("t1", "landed", 10)]),
    reviews: byId([review("v1", 20, { type: "findings", count: 1 }, [{ id: "R1.1", severity: "P2", title: "A bug", location: null, status: { type: "dismissed", why: "fine" }, high: false }])]),
    orchestratorSteps: [
      step(40, { type: "ciWatch", repo: "o/r", branch: "b", sha: "abc", waiting: false, runs: [], result: "passed" }),
      step(50, { type: "merged", branch: "b", base: "main", commits: 1, askedIn: null, mergeCommit: "1234567890", record }),
    ],
  };
  const jobs = sessionJobs(input(parts));
  assert.equal(jobs.current, null);
  assert.equal(jobs.past.length, 1);
  const past = jobs.past[0]!;
  assert.deepEqual(past.steps.map((s) => `${s.id}:${s.state}`), ["build:done", "review:done", "fix:done", "ci:done", "merge:done"]);
  assert.equal(past.steps.at(-1)?.summary, "Merged 1234567 into main");
  assert.equal(past.steps.find((s) => s.id === "fix")?.summary, "1 dismissed");
  assert.equal(past.facts.findings.length, 1);
  assert.equal(past.facts.merge?.record, record);
  // A worker after the merge starts the next job.
  assert.deepEqual(steps({ ...parts, tasks: { ...parts.tasks, ...byId([task("t2", "running", 60)]) } }), ["build:active", "review:pending", "checks:pending", "merge:pending"]);
});

test("a plan of phases shows its phase at work, and stays while phases are left", () => {
  const plan = {
    id: "p1", conversationId: "c", requestId: "r1", position: 0, title: "Windows", body: "# Windows", state: { type: "approved" }, createdAtMs: 1, decidedAtMs: 2,
    steps: [
      { title: "Paths", detail: null, taskId: "t1", stage: "done", startedAtMs: 5, endedAtMs: 20, outline: null },
      { title: "Installer", detail: null, taskId: "t2", stage: "building", startedAtMs: 30, endedAtMs: null, outline: null },
      { title: "Docs", detail: null, taskId: null, stage: "pending", startedAtMs: null, endedAtMs: null, outline: null },
    ],
  } as unknown as Plan;
  const requests = { r1: { id: "r1", steeredInto: null, startedAtMs: 1, state: { type: "working" } } as unknown as UserRequest };
  const parts = { plans: { p1: plan }, requests, tasks: byId([task("t1", "landed", 5), task("t2", "running", 30)]), reviews: byId([review("v1", 21, { type: "clean" })]) };
  const job = sessionJobs(input(parts)).current!;
  assert.deepEqual(job.phase, { number: 2, of: 3, title: "Installer" });
  // Phase 1's review is before the phase: not this phase's.
  assert.deepEqual(job.steps.map((s) => `${s.id}:${s.state}`), ["build:active", "review:pending", "checks:pending", "merge:pending"]);
  assert.equal(jobLabel(job), "Phase 2 of 3 · Building");
});

test("an overnight run at work no longer hides the session's job: the Job card's slot picks between them (runInSlot)", () => {
  const run = { id: "o1", state: "running", planId: null } as unknown as OvernightRun;
  assert.equal(jobLabel(sessionJobs(input({ tasks: byId([task("t1", "running", 10)]), overnight: { o1: run } })).current!), "Building");
});

// Issue 414: the thread fixed a finding itself and the checks run before it records it: the
// checks are the one step at work, Fixing behind them is passed.
test("one step is at work at a time, and the job never goes back to an earlier one", () => {
  const finding = { id: "R1.1", severity: "P3", title: "Years below 100", location: null, status: { type: "new" }, high: false } as ReviewRun["items"][number];
  const parts = {
    tasks: byId([task("t1", "landed", 10)]),
    reviews: byId([review("v1", 20, { type: "findings", count: 1 }, [finding])]),
    integratedChecks: byId([check(1, 30, { type: "running", command: "npm test", done: 0, total: 1 })]),
  };
  assert.deepEqual(steps(parts), ["build:done", "review:done", "fix:done", "checks:active", "merge:pending"]);
  // While a review still runs, an earlier review's finding waits: Reviewing is the step at work.
  const reviewing = { tasks: parts.tasks, reviews: byId([review("v1", 20, { type: "findings", count: 1 }, [finding]), review("v2", 25, { type: "running" })]) };
  assert.deepEqual(steps(reviewing), ["build:done", "review:active", "fix:pending", "checks:pending", "merge:pending"]);
  // Issue 423: a worker started once the reviews began is a fix, not Building again.
  const after = { ...parts, integratedChecks: {}, tasks: { ...parts.tasks, ...byId([task("t2", "running", 40)]) }, reviews: byId([review("v1", 20, { type: "clean" })]) };
  assert.deepEqual(steps(after), ["build:done", "review:done", "fix:active", "checks:pending", "merge:pending"]);
});

// Issue 423: a worker that only looks at the running app after the merge changes nothing: no new
// job at Building.
test("a live look after a merge starts no job", () => {
  const parts = {
    tasks: byId([task("t1", "landed", 10), task("t2", "running", 60, { liveLook: true })]),
    orchestratorSteps: [step(50, { type: "merged", branch: "b", base: "main", commits: 1, askedIn: null, mergeCommit: "1234567890" })],
  };
  assert.equal(sessionJobs(input(parts)).current, null);
});
