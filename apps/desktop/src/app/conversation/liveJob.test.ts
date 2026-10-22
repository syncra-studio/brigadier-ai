import assert from "node:assert/strict";
import { test } from "node:test";

import { type LiveInput, liveJob } from "@/app/conversation/liveJob";
import type { ShownApproval } from "@/state/board";
import type { CiRun, OrchestratorStep, Preview, Question, Task, UserRequest } from "@/ipc/generated";

const request = (id: string, startedAtMs: number, state: UserRequest["state"], endedAtMs: number | null = null): UserRequest =>
  ({ id, conversationId: "c", preview: "", state, startedAtMs, endedAtMs, steeredInto: null, steeredAfter: null, undo: null, worked: [] }) as unknown as UserRequest;

const step = (atMs: number, kind: OrchestratorStep["kind"]): OrchestratorStep => ({ requestId: "r1", kind, atMs, position: 0 });

const run = (workflow: string, status: string, conclusion: string, jobsDone = 0, jobsTotal = 0): CiRun =>
  ({ workflow, url: `https://ci/${workflow}`, status, conclusion, jobsDone, jobsTotal }) as unknown as CiRun;

const ci = (atMs: number, runs: CiRun[], result?: string): OrchestratorStep =>
  step(atMs, { type: "ciWatch", repo: "o/r", branch: "main", sha: "4e1c49e99aa", waiting: runs.length === 0, runs, ...(result ? { result } : {}) } as OrchestratorStep["kind"]);

const question = (id: string, createdAtMs: number, kind: Question["kind"]["type"] = "orchestrator"): Question =>
  ({ id, kind: { type: kind }, answer: null, answeredAtMs: null, answers: [], createdAtMs }) as unknown as Question;

const byId = <T extends { id: string }>(items: T[]) => Object.fromEntries(items.map((item) => [item.id, item]));

const input = (parts: Partial<LiveInput>): LiveInput => ({
  tasks: {}, approvals: {}, questions: {}, previews: {}, orchestratorSteps: [], requests: {},
  ...parts,
});

/** A turn that ended at 50: nothing runs. */
const idle = { requests: byId([request("r1", 10, { type: "done" }, 50)]) };
/** The thread at work since 10. */
const working = { requests: byId([request("r1", 10, { type: "working" })]) };

const words = (parts: Partial<LiveInput>) => {
  const job = liveJob(input(parts));
  return job && [job.tone, job.headline, job.detail ?? null];
};

test("with nothing running and nothing waiting there is no card; while the thread works it says Working, naming no worker", () => {
  assert.equal(liveJob(input({})), null);
  assert.equal(liveJob(input(idle)), null);
  const tasks = byId([{ id: "t1", title: "Shared MCP and skills", state: "running", requestId: "r1", createdAtMs: 20, updatedAtMs: 20 } as unknown as Task]);
  assert.deepEqual(words({ ...working, tasks }), ["running", "Working", null]);
  assert.equal(liveJob(input({ ...working, tasks }))!.sinceMs, 10);
  // A worker at work after the thread's turn ended: still Working, from when it started.
  assert.deepEqual(words({ ...idle, tasks }), ["running", "Working", null]);
  assert.equal(liveJob(input({ ...idle, tasks }))!.sinceMs, 20);
});

test("merge: merging, then merged", () => {
  const merged = step(80, { type: "merged", branch: "s", base: "main", commits: 3, askedIn: null, head: "head111", mergeCommit: "4e1c49e000" } as OrchestratorStep["kind"]);
  assert.deepEqual(words({ ...idle, orchestratorSteps: [merged] }), ["merged", "Merged into main", "4e1c49e · 3 commits"]);
  const requested = step(60, { type: "mergeRequested", id: "m", branch: "s", base: "main", origin: {} } as unknown as OrchestratorStep["kind"]);
  assert.deepEqual(words({ ...working, orchestratorSteps: [requested] }), ["running", "Merging into main", "s into main"]);
  // A click with no outcome once its work is over (turned down, or stopped) merges nothing.
  assert.equal(words({ ...idle, orchestratorSteps: [requested] }), null);
  const stopped = { requests: byId([request("r1", 10, { type: "stopped" }, 70)]) };
  assert.deepEqual(words({ ...stopped, orchestratorSteps: [requested] }), ["stopped", "Stopped", "You stopped it"]);
  // Nor while the user's next message runs.
  const next = { requests: byId([request("r1", 10, { type: "done" }, 70), request("r2", 100, { type: "working" })]) };
  assert.equal(words({ ...next, orchestratorSteps: [requested] })![1], "Working");
});

test("CI in words; its outcome stays until the next message", () => {
  const checking = liveJob(input({ ...working, orchestratorSteps: [ci(30, [run("test", "in_progress", "", 3, 5)])] }))!;
  assert.deepEqual([checking.headline, checking.detail], ["Checking CI · 3 of 5 jobs done", "main · 4e1c49e9"]);
  // Long after it ended (no 4-second linger), the outcome holds until the user writes.
  const failed = [ci(30, [run("test", "completed", "failure")], "failed")];
  assert.deepEqual(words({ ...idle, orchestratorSteps: failed }), ["failed", "CI failed · test", "main · 4e1c49e9"]);
  assert.deepEqual(words({ ...idle, orchestratorSteps: [ci(30, [run("test", "completed", "success")], "passed")] })![1], "CI passed");
  const next = { requests: byId([request("r1", 10, { type: "done" }, 50), request("r2", 90, { type: "done" }, 95)]) };
  assert.equal(liveJob(input({ ...next, orchestratorSteps: [ci(30, [run("test", "completed", "success")], "passed")] })), null);
});

test("waiting on you points at the cards below, never copying them, and comes before anything else", () => {
  const questions = byId([question("q1", 40)]);
  const busy = { ...working, questions, orchestratorSteps: [ci(20, [run("test", "in_progress", "", 3, 5)])] };
  assert.deepEqual(words(busy), ["waiting", "Waiting for your answer below", null]);
  assert.deepEqual(words({ ...working, questions: byId([question("q1", 40, "merge")]) })!.slice(0, 2), ["waiting", "Waiting for your yes below"]);
  const approvals = byId([{ id: "a1", state: { type: "pending" }, createdAtMs: 40, subject: { type: "cli" } } as unknown as ShownApproval]);
  assert.deepEqual(words({ ...working, approvals })!.slice(0, 2), ["waiting", "Waiting for your approval below"]);
  // An answered one waits no more.
  assert.deepEqual(words({ ...working, questions: byId([{ ...question("q1", 40), answer: "Yes", answeredAtMs: 45 }]) })![1], "Working");
});

test("failures in the work: a worker that failed, a dev server that died; before running and done", () => {
  const tasks = byId([{ id: "t1", title: "Backend", state: "failed", error: "Tests failed\nmore", requestId: "r1", createdAtMs: 20, updatedAtMs: 40 } as unknown as Task]);
  assert.deepEqual(words({ ...working, tasks }), ["failed", "A worker failed", "Tests failed"]);
  const previews = byId([
    { id: "p1", name: "The dev server", state: { type: "exited", code: 1, status: "Port 1420 is already in use" }, startedAtMs: 20, endedAtMs: 30 } as unknown as Preview,
  ]);
  assert.deepEqual(words({ ...working, previews }), ["failed", "The dev server stopped", "Port 1420 is already in use"]);
  // Restarted since: up again, nothing failed.
  const again = byId([...Object.values(previews), { id: "p2", name: "The dev server", state: { type: "running" }, startedAtMs: 35, endedAtMs: null } as unknown as Preview]);
  assert.deepEqual(words({ ...working, previews: again })![1], "Working");
});

test("a finished outcome stays until the next message; what still runs stays whenever it began", () => {
  const passed = [ci(40, [run("test", "completed", "success")], "passed")];
  assert.equal(words({ ...idle, orchestratorSteps: passed })![1], "CI passed");
  const next = { requests: byId([request("r1", 10, { type: "done" }, 50), request("r2", 60, { type: "working" })]) };
  assert.equal(words({ ...next, orchestratorSteps: passed })![1], "Working");
  const checking = [ci(20, [run("test", "in_progress", "", 3, 5)])];
  assert.equal(words({ ...next, orchestratorSteps: checking })![1], "Checking CI · 3 of 5 jobs done");
  // While the thread goes on, an outcome since the last message says more than Working.
  assert.equal(words({ ...working, orchestratorSteps: passed })![1], "CI passed");
  // Stopped: stays until the next message.
  assert.deepEqual(words({ requests: byId([request("r1", 10, { type: "stopped" }, 50)]) }), ["stopped", "Stopped", "You stopped it"]);
});

test("the thread's headline wins until something that needs the user, or a failure, comes after it", () => {
  const headline = { text: "Investigating the dev server", detail: "Looking for what holds port 1420", failed: false, atMs: 40 };
  const tasks = byId([{ id: "t1", kind: "implement", state: "running", requestId: "r1", createdAtMs: 20, updatedAtMs: 20 } as unknown as Task]);
  const job = liveJob(input({ ...working, tasks, headline }))!;
  assert.deepEqual([job.tone, job.headline, job.detail], ["running", "Investigating the dev server", "Looking for what holds port 1420"]);
  // A failure after it: the failure shows.
  const failed = byId([{ id: "t2", title: "Backend", state: "failed", error: "Tests failed", requestId: "r1", createdAtMs: 45, updatedAtMs: 50 } as unknown as Task]);
  assert.equal(words({ ...working, headline, tasks: failed })![1], "A worker failed");
  // A question after it: the card points at it.
  assert.equal(words({ ...working, headline, questions: byId([question("q1", 60)]) })![1], "Waiting for your answer below");
  // Its outcome words once nothing runs: a tick, until the next message.
  assert.deepEqual(words({ ...idle, headline: { text: "The dev server is up again", detail: null, failed: false, atMs: 45 } }), ["done", "The dev server is up again", null]);
  assert.equal(liveJob(input({ ...idle, headline: { text: "Old", detail: null, failed: false, atMs: 5 } })), null);
});

test("a success never carries the failure mark: a failure from before the thread's words is the thread's to judge", () => {
  // Merged, then CI that GitHub marked cancelled (Windows timed out saving its build cache), then
  // the thread's words that it passed: the merge's mark, not a failure's.
  const merged = step(30, { type: "merged", branch: "s", base: "main", commits: 3, askedIn: null, head: "head111", mergeCommit: "4e1c49e000" } as OrchestratorStep["kind"]);
  const cancelled = ci(40, [run("CI", "completed", "cancelled", 6, 6)], "failed");
  const passes = { text: "Merged, pushed, CI passes", detail: "Windows only timed out saving its build cache", failed: false, atMs: 45 };
  assert.deepEqual(words({ ...idle, orchestratorSteps: [merged, cancelled], headline: passes }), ["merged", "Merged, pushed, CI passes", "Windows only timed out saving its build cache"]);
  // With no other outcome, the thread's words are a tick.
  assert.equal(liveJob(input({ ...idle, orchestratorSteps: [cancelled], headline: passes }))!.tone, "done");
  // Words that tell of a failure that still stands: the failure mark.
  assert.equal(liveJob(input({ ...idle, orchestratorSteps: [cancelled], headline: { ...passes, text: "CI failed on Windows", failed: true } }))!.tone, "failed");
  // A cancelled run after the thread's words: it shows as cancelled, words and mark alike, never as a failure.
  assert.deepEqual(words({ ...idle, orchestratorSteps: [merged, cancelled], headline: { ...passes, atMs: 35 } }), ["stopped", "CI cancelled · CI", "main · 4e1c49e9"]);
  // A failed one: the failure shows, mark and all.
  assert.deepEqual(words({ ...idle, orchestratorSteps: [merged, ci(40, [run("test", "completed", "failure")], "failed")], headline: { ...passes, atMs: 35 } })!.slice(0, 2), ["failed", "CI failed · test"]);
});
