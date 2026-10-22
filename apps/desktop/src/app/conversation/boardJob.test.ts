import assert from "node:assert/strict";
import { test } from "node:test";

import { type JobInput, sessionJobs } from "@/app/conversation/boardJob";
import { jobLabel } from "@/app/conversation/job";
import type { OrchestratorStep, OvernightRun, Plan, Task, TaskState, UserRequest } from "@/ipc/generated";

const task = (id: string, state: TaskState, createdAtMs: number, extra: Partial<Task> = {}): Task =>
  ({ id, number: 1, title: `Task ${id}`, kind: "implement", state, requestId: "r1", subject: null, createdAtMs, updatedAtMs: createdAtMs + 5, ...extra }) as unknown as Task;

const step = (atMs: number, kind: OrchestratorStep["kind"]): OrchestratorStep => ({ requestId: "r1", kind, atMs, position: 0 });

const input = (parts: Partial<JobInput>): JobInput => ({
  tasks: {}, orchestratorSteps: [], plans: {}, requests: {}, overnight: {}, ...parts,
});
const byId = <T extends { id: string }>(items: T[]) => Object.fromEntries(items.map((item) => [item.id, item]));
const steps = (parts: Partial<JobInput>) => sessionJobs(input(parts)).current?.steps.map((s) => `${s.id}:${s.state}`);

test("a question has no job; a worker at work is Building, with nothing of Brigadier's to come", () => {
  assert.equal(sessionJobs(input({})).current, null);
  const parts = { tasks: byId([task("t1", "running", 10)]) };
  assert.deepEqual(steps(parts), ["build:active"]);
  assert.equal(jobLabel(sessionJobs(input(parts)).current!), "Building");
});

test("a change all done, nothing at work, has no job; idle with a failure, the failure stays", () => {
  const done = { tasks: byId([task("t1", "landed", 10)]) };
  assert.equal(sessionJobs(input(done)).current, null);
  const failed = { ...done, orchestratorSteps: [step(30, { type: "ciWatch", repo: "o/r", branch: "b", sha: "abc", waiting: false, runs: [], result: "failed" })] };
  assert.deepEqual(steps(failed), ["build:done", "ci:failed"]);
  assert.equal(jobLabel(sessionJobs(input(failed)).current!), "CI failed");
});

test("a merge closes the job: the tab keeps it", () => {
  const parts = {
    tasks: byId([task("t1", "landed", 10)]),
    orchestratorSteps: [
      step(40, { type: "ciWatch", repo: "o/r", branch: "b", sha: "abc", waiting: false, runs: [], result: "passed" }),
      step(50, { type: "merged", branch: "b", base: "main", commits: 1, askedIn: null, mergeCommit: "1234567890" }),
    ],
  };
  const jobs = sessionJobs(input(parts));
  assert.equal(jobs.current, null);
  assert.equal(jobs.past.length, 1);
  const past = jobs.past[0]!;
  assert.deepEqual(past.steps.map((s) => `${s.id}:${s.state}`), ["build:done", "ci:done", "merge:done"]);
  assert.equal(past.steps.at(-1)?.summary, "Merged 1234567 into main");
  // A worker after the merge starts the next job.
  assert.deepEqual(steps({ ...parts, tasks: { ...parts.tasks, ...byId([task("t2", "running", 60)]) } }), ["build:active"]);
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
  const parts = { plans: { p1: plan }, requests, tasks: byId([task("t1", "landed", 5), task("t2", "running", 30)]) };
  const job = sessionJobs(input(parts)).current!;
  assert.deepEqual(job.phase, { number: 2, of: 3, title: "Installer" });
  assert.deepEqual(job.steps.map((s) => `${s.id}:${s.state}`), ["build:active"]);
  assert.equal(jobLabel(job), "Phase 2 of 3 · Building");
});

test("an overnight run at work no longer hides the session's job: the Job card's slot picks between them (runInSlot)", () => {
  const run = { id: "o1", state: "running", planId: null } as unknown as OvernightRun;
  assert.equal(jobLabel(sessionJobs(input({ tasks: byId([task("t1", "running", 10)]), overnight: { o1: run } })).current!), "Building");
});

// Issue 414: one step at work at a time: CI at work after the build, Building behind it is passed.
test("one step is at work at a time, and the job never goes back to an earlier one", () => {
  const parts = {
    tasks: byId([task("t1", "running", 10)]),
    orchestratorSteps: [step(30, { type: "ciWatch", repo: "o/r", branch: "b", sha: "abc", waiting: false, runs: [] })],
  };
  assert.deepEqual(steps(parts), ["build:done", "ci:active"]);
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
