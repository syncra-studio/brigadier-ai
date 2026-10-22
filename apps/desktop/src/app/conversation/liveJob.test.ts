import assert from "node:assert/strict";
import { test } from "node:test";

import { type LiveInput, liveJob } from "@/app/conversation/liveJob";
import type { ShownApproval } from "@/state/board";
import type { CiRun, IntegratedCheck, IntegratedCheckState, OrchestratorStep, Preview, Question, ReviewRun, ReviewState, Task, UserRequest } from "@/ipc/generated";

const request = (id: string, startedAtMs: number, state: UserRequest["state"], endedAtMs: number | null = null): UserRequest =>
  ({ id, conversationId: "c", preview: "", state, startedAtMs, endedAtMs, steeredInto: null, steeredAfter: null, undo: null, worked: [] }) as unknown as UserRequest;

/** A `run_checks` run: the thread's full checks unless `asked` says otherwise (null: one Brigadier started itself). */
const check = (
  n: number,
  tip: string,
  state: IntegratedCheckState,
  startedAtMs = 100,
  endedAtMs: number | null = null,
  asked: { taskId: string | null; scope: "full" | "change" } | null = { taskId: null, scope: "full" },
): IntegratedCheck =>
  ({ id: `check-${n}`, conversationId: "c", repo: "/r", branch: "b", tip, state, dueAtMs: startedAtMs, startedAtMs, endedAtMs, requests: ["r1"], tasks: [], rerun: false, ...(asked ? { asked } : {}) }) as IntegratedCheck;

const review = (id: string, startedAtMs: number, state: ReviewState, endedAtMs: number | null = null): ReviewRun =>
  ({ id, conversationId: "c", requestId: "r1", taskId: null, kind: "code", base: "aaaaaaa000", tip: "4e1c49e999", author: "claude", reviewer: "codex", state, startedAtMs, endedAtMs, number: 1, items: [] }) as unknown as ReviewRun;

const step = (atMs: number, kind: OrchestratorStep["kind"]): OrchestratorStep => ({ requestId: "r1", kind, atMs, position: 0 });

const run = (workflow: string, status: string, conclusion: string, jobsDone = 0, jobsTotal = 0): CiRun =>
  ({ workflow, url: `https://ci/${workflow}`, status, conclusion, jobsDone, jobsTotal }) as unknown as CiRun;

const ci = (atMs: number, runs: CiRun[], result?: string): OrchestratorStep =>
  step(atMs, { type: "ciWatch", repo: "o/r", branch: "main", sha: "4e1c49e99aa", waiting: runs.length === 0, runs, ...(result ? { result } : {}) } as OrchestratorStep["kind"]);

const question = (id: string, createdAtMs: number, kind: Question["kind"]["type"] = "orchestrator"): Question =>
  ({ id, kind: { type: kind }, answer: null, answeredAtMs: null, answers: [], createdAtMs }) as unknown as Question;

const byId = <T extends { id: string }>(items: T[]) => Object.fromEntries(items.map((item) => [item.id, item]));

const input = (parts: Partial<LiveInput>): LiveInput => ({
  tasks: {}, approvals: {}, questions: {}, reviews: {}, previews: {}, integratedChecks: {}, orchestratorSteps: [], requests: {},
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

test("checks: running with the command at work, failed with what failed, passed on the commit", () => {
  assert.deepEqual(words({ ...working, integratedChecks: byId([check(1, "4e1c49e999", { type: "running", command: "desktop tests", done: 0, total: 1 })]) }), [
    "running",
    "Running checks",
    "Now: desktop tests",
  ]);
  assert.deepEqual(words({ ...idle, integratedChecks: byId([check(1, "4e1c49e999", { type: "failed", command: "pnpm test", status: "exit 1", output: null }, 100, 200)]) }), [
    "failed",
    "Checks failed",
    "pnpm test: exit 1",
  ]);
  assert.deepEqual(words({ ...idle, integratedChecks: byId([check(1, "4e1c49e999", { type: "passed", commands: ["a", "b", "c", "d", "e"] }, 100, 200)]) }), [
    "done",
    "Checks passed on 4e1c49e",
    "5 of 5 passed",
  ]);
  // run_checks counts its commands: the one at work is the third of five.
  const counted: IntegratedCheckState = { type: "running", command: "desktop tests", done: 2, total: 5 };
  assert.deepEqual(words({ ...working, integratedChecks: byId([check(1, "4e1c49e999", counted)]) })!.slice(0, 2), ["running", "Running checks · 3 of 5"]);
  // A worker's own checks are its row's, not the card's.
  const workers = byId([check(1, "4e1c49e999", counted, 100, null, { taskId: "t1", scope: "change" })]);
  assert.deepEqual(words({ ...working, integratedChecks: workers })![1], "Working");
  // Only the newest run counts: a pass after a failure is the outcome.
  const both = byId([check(1, "aaa", { type: "failed", command: "x", status: "1", output: null }, 60, 70), check(2, "bbbbbbb1", { type: "passed", commands: ["a"] }, 80, 90)]);
  assert.equal(words({ ...idle, integratedChecks: both })![1], "Checks passed on bbbbbbb");
});

test("review: who reviews while it runs, then what it found, or that it is clean", () => {
  assert.deepEqual(words({ ...working, reviews: byId([review("v1", 20, { type: "running" })]) }), ["running", "Codex is reviewing", "The changes up to 4e1c49e"]);
  assert.deepEqual(words({ ...idle, reviews: byId([review("v1", 60, { type: "findings", count: 3 }, 70)]) }), [
    "done",
    "Review found 3 issues",
    "From Codex’s review of 4e1c49e",
  ]);
  assert.deepEqual(words({ ...idle, reviews: byId([review("v1", 60, { type: "clean" }, 70)]) })![1], "Review found nothing to fix");
});

test("merge: merging, merged with checks passed only on the merged commit, never claiming a pass otherwise", () => {
  const merged = (head: string) =>
    step(80, { type: "merged", branch: "s", base: "main", commits: 3, askedIn: null, head, mergeCommit: "4e1c49e000" } as OrchestratorStep["kind"]);
  const passed = byId([check(1, "head111", { type: "passed", commands: ["a"] }, 60, 70)]);
  assert.deepEqual(words({ ...idle, integratedChecks: passed, orchestratorSteps: [merged("head111")] }), [
    "merged",
    "Merged into main · checks passed",
    "4e1c49e · 3 commits",
  ]);
  // Merged anyway, or with checks on another commit: no claim.
  assert.equal(words({ ...idle, integratedChecks: passed, orchestratorSteps: [merged("other22")] })![1], "Merged into main");
  // Nor a pass of checks Brigadier ran by itself, or of a worker's change only.
  const unasked = byId([check(1, "head111", { type: "passed", commands: ["a"] }, 60, 70, null)]);
  assert.equal(words({ ...idle, integratedChecks: unasked, orchestratorSteps: [merged("head111")] })![1], "Merged into main");
  const requested = step(60, { type: "mergeRequested", id: "m", branch: "s", base: "main", origin: {} } as unknown as OrchestratorStep["kind"]);
  assert.deepEqual(words({ ...working, integratedChecks: passed, orchestratorSteps: [requested] }), ["running", "Merging into main", "Checks passed on head111"]);
  // A click with no outcome once its work is over (turned down, or stopped) merges nothing.
  assert.equal(words({ ...idle, orchestratorSteps: [requested] }), null);
  const stopped = { requests: byId([request("r1", 10, { type: "stopped" }, 70)]) };
  assert.deepEqual(words({ ...stopped, orchestratorSteps: [requested] }), ["stopped", "Stopped", "You stopped it"]);
  // Nor while the user's next message runs.
  const next = { requests: byId([request("r1", 10, { type: "done" }, 70), request("r2", 100, { type: "working" })]) };
  assert.equal(words({ ...next, orchestratorSteps: [requested] })![1], "Working");
  // Merged anyway: it says the checks were skipped, even with a pass on that commit from before.
  const skipped = step(80, { type: "merged", branch: "s", base: "main", commits: 3, askedIn: null, head: "head111", mergeCommit: "4e1c49e000", checksSkipped: true } as OrchestratorStep["kind"]);
  assert.equal(words({ ...idle, integratedChecks: passed, orchestratorSteps: [skipped] })![1], "Merged into main · checks skipped");
  const anyway = step(60, { type: "mergeRequested", id: "m", branch: "s", base: "main", origin: {}, anyway: true } as unknown as OrchestratorStep["kind"]);
  assert.deepEqual(words({ ...working, integratedChecks: passed, orchestratorSteps: [anyway] }), ["running", "Merging into main", "Without the checks, as you said"]);
});

/** The merge rule refused the merge at 30: no run_checks pass on its commit. */
const refused = (waivable: boolean, atMs = 30) => ({ reason: "The checks haven't run on 7d2a91f, the tip of s.", sha: "7d2a91f000", branch: "s", base: "main", waivable, atMs });

test("a merge the merge rule refused: why, waiting on the user only when they can waive it", () => {
  const job = liveJob(input({ ...working, mergeRefused: refused(true) }))!;
  assert.deepEqual([job.tone, job.headline, job.detail], ["waiting", "Can’t merge yet", "The checks haven't run on 7d2a91f, the tip of s."]);
  // It waits on the user before what runs: the checks the thread runs now don't hide it.
  const running = byId([check(1, "7d2a91f000", { type: "running", command: "pnpm test", done: 0, total: 2 }, 40)]);
  assert.equal(liveJob(input({ ...working, integratedChecks: running, mergeRefused: refused(true) }))!.headline, "Can’t merge yet");
  // One the user can’t waive: the words.
  const firm = liveJob(input({ ...working, mergeRefused: refused(false) }))!;
  assert.deepEqual([firm.headline, firm.tone], ["Can’t merge yet", "waiting"]);
  // Over once the user wrote since, or a merge went through after it.
  assert.equal(liveJob(input({ requests: byId([request("r2", 40, { type: "done" }, 50)]), mergeRefused: refused(true) })), null);
  const merged = step(60, { type: "merged", branch: "s", base: "main", commits: 3, askedIn: null, head: "7d2a91f000", mergeCommit: "4e1c49e000" } as OrchestratorStep["kind"]);
  assert.equal(liveJob(input({ ...idle, orchestratorSteps: [merged], mergeRefused: refused(true) }))!.headline, "Merged into main");
  // Newer than the thread's headline, it wins over it.
  const headline = { text: "Fixing the tests", detail: null, failed: false, atMs: 20 };
  assert.equal(liveJob(input({ ...working, headline, mergeRefused: refused(true) }))!.headline, "Can’t merge yet");
  const later = liveJob(input({ ...working, headline: { ...headline, atMs: 40 }, mergeRefused: refused(true) }))!;
  assert.deepEqual([later.tone, later.headline], ["waiting", "Fixing the tests"]);
});

test("the held merge and CI in words; CI's outcome stays until the next message", () => {
  const held = step(30, { type: "mergeHeld", branch: "s", base: "main", askedIn: "r1", consentedAtMs: 30, condition: { type: "workersDone" }, waiting: ["t1"], tip: "abc" } as unknown as OrchestratorStep["kind"]);
  const tasks = byId([{ id: "t1", kind: "implement", state: "running", requestId: "r1", createdAtMs: 20, updatedAtMs: 20 } as unknown as Task]);
  const job = liveJob(input({ ...working, tasks, orchestratorSteps: [held] }))!;
  assert.deepEqual([job.headline, job.detail], ["Merging after 1 worker finishes", "Into main once they’re done, as you asked"]);

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
  const busy = { ...working, questions, integratedChecks: byId([check(1, "abc", { type: "running", command: "x", done: 0, total: 1 })]) };
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
  const passed = byId([check(1, "abc", { type: "passed", commands: ["a"] }, 20, 40)]);
  assert.equal(words({ ...idle, integratedChecks: passed })![1], "Checks passed on abc");
  const next = { requests: byId([request("r1", 10, { type: "done" }, 50), request("r2", 60, { type: "working" })]) };
  assert.equal(words({ ...next, integratedChecks: passed })![1], "Working");
  const running = byId([check(1, "abc", { type: "running", command: "x", done: 0, total: 1 }, 20)]);
  assert.equal(words({ ...next, integratedChecks: running })![1], "Running checks");
  // While the thread goes on, an outcome since the last message says more than Working.
  assert.equal(words({ ...working, integratedChecks: passed })![1], "Checks passed on abc");
  // Stopped: stays until the next message.
  assert.deepEqual(words({ requests: byId([request("r1", 10, { type: "stopped" }, 50)]) }), ["stopped", "Stopped", "You stopped it"]);
});

test("the thread's headline wins until something that needs the user, or a failure, comes after it", () => {
  const headline = { text: "Investigating the dev server", detail: "Looking for what holds port 1420", failed: false, atMs: 40 };
  const held = step(30, { type: "mergeHeld", branch: "s", base: "main", askedIn: "r1", consentedAtMs: 30, condition: { type: "workersDone" }, waiting: ["t1"], tip: "abc" } as unknown as OrchestratorStep["kind"]);
  const tasks = byId([{ id: "t1", kind: "implement", state: "running", requestId: "r1", createdAtMs: 20, updatedAtMs: 20 } as unknown as Task]);
  const job = liveJob(input({ ...working, tasks, orchestratorSteps: [held], headline }))!;
  assert.deepEqual([job.tone, job.headline, job.detail], ["running", "Investigating the dev server", "Looking for what holds port 1420"]);
  // A failure after it: the failure shows.
  const failed = byId([check(1, "abc", { type: "failed", command: "pnpm test", status: "exit 1", output: null }, 45, 50)]);
  assert.equal(words({ ...working, headline, integratedChecks: failed })![1], "Checks failed");
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
  // A failure after the thread's words: the failure shows, mark and all.
  assert.deepEqual(words({ ...idle, orchestratorSteps: [merged, cancelled], headline: { ...passes, atMs: 35 } })!.slice(0, 1), ["failed"]);
});
