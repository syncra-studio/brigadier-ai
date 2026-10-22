import assert from "node:assert/strict";
import { test } from "node:test";

import { appCommand, type BackgroundEvents, backgroundRows } from "@/app/conversation/backgroundRows";
import type { CiRun, OrchestratorStep, Preview, PreviewState, Task, UserRequest } from "@/ipc/generated";

const preview = (id: string, name: string, command: string, startedAtMs: number, state: PreviewState = { type: "running" }, extra: Partial<Preview> = {}): Preview =>
  ({ id, conversationId: "c", name, command, workdir: "/w", workspace: "/w", pid: 1, state, startedAtMs, endedAtMs: null, log: null, kind: "web", building: false, ...extra }) as Preview;

const rows = (previews: Preview[], tasks: Record<string, Task> = {}) =>
  backgroundRows(previews, tasks, () => null).map((row) => [row.type === "preview" ? row.preview.name : row.type === "event" ? row.name : row.id, row.state]);

const exited = (code: number | null) => ({ type: "exited", code, status: code === null ? "killed by signal 9" : `exit ${code}` }) as PreviewState;

test("a worker's smoke test of the running app is a state of the app's row, not a row of its own", () => {
  const app = preview("preview-4", "todo app", 'PORT=3000 TODO_FILE="$TMPDIR/todos.json" node src/server.js', 10);
  const smoke = preview("preview-5", "todo smoke", 'PORT=38471 TODO_FILE="/scratch/t.json" node src/server.js', 20, { type: "running" }, { taskId: "t3" });
  const look = { t3: { id: "t3", liveLook: true } as Task };
  assert.deepEqual(rows([app]), [["todo app", "running"]]);
  assert.deepEqual(rows([app, smoke], look), [["todo app", "smoke testing"]]);
  // Any other worker's copy checks against it; a browser at it smoke-tests it.
  assert.deepEqual(rows([app, { ...smoke, name: "todo server" }]), [["todo app", "checking"]]);
  const browser = preview("preview-6", "Todo browser smoke", "chrome-headless-shell --remote-debugging-port=43987 http://localhost:3000/", 30);
  assert.deepEqual(rows([app, browser]), [["todo app", "smoke testing"]]);
  // A copy that ended leaves the app running again.
  assert.deepEqual(rows([app, { ...smoke, state: { type: "stopped", reason: "stopped by its worker" } }], look), [["todo app", "running"]]);
});

test("a row's state follows its process: building, starting, running, paused", () => {
  assert.deepEqual(rows([preview("p", "Dev app", "pnpm tauri dev", 1, { type: "starting" }, { building: true })]), [["Dev app", "building"]]);
  assert.deepEqual(rows([preview("p", "Dev app", "pnpm dev", 1, { type: "starting" })]), [["Dev app", "starting"]]);
  assert.deepEqual(rows([preview("p", "Dev app", "pnpm dev", 1, { type: "paused" })]), [["Dev app", "paused"]]);
  assert.equal(appCommand("PORT=1 A='b c' node x.js"), "node x.js");
});

test("a row leaves as soon as its process ends: failed, finished or stopped alike", () => {
  const crashed = preview("preview-1", "Dev app", "pnpm dev", 10, exited(1), { endedAtMs: 50 });
  const done = preview("preview-2", "Docs build", "pnpm docs", 20, exited(0), { endedAtMs: 60 });
  const stopped = preview("preview-3", "Web app", "pnpm web", 30, { type: "stopped", reason: "stopped by the user" }, { endedAtMs: 70 });
  const paused = preview("preview-4", "API", "node api.js", 40, { type: "paused" });
  // Paused, it is still there; a non-zero exit, a crash, a clean exit or a Stop: gone.
  assert.deepEqual(rows([crashed, done, stopped, paused]), [["API", "paused"]]);
  assert.deepEqual(rows([{ ...crashed, state: exited(null) }]), []);
  // Started again under its id, it is a running row again.
  assert.deepEqual(rows([{ ...crashed, state: { type: "starting" } }]), [["Dev app", "starting"]]);
});

const request = (id: string, startedAtMs: number, state: UserRequest["state"]): UserRequest =>
  ({ id, conversationId: "c", preview: "", state, startedAtMs, endedAtMs: null, steeredInto: null, steeredAfter: null, undo: null, worked: [] }) as unknown as UserRequest;
const step = (atMs: number, kind: unknown, requestId = "r1"): OrchestratorStep => ({ requestId, kind: kind as OrchestratorStep["kind"], atMs, position: 0 });
const ciRun = (status: string, conclusion: string, jobsDone: number, jobsTotal: number): CiRun =>
  ({ workflow: "test", url: "https://ci/test", status, conclusion, jobsDone, jobsTotal }) as unknown as CiRun;
const ci = (atMs: number, runs: CiRun[], result?: string) =>
  step(atMs, { type: "ciWatch", repo: "o/r", branch: "main", sha: "4e1c49e99aa", waiting: runs.length === 0, runs, ...(result ? { result } : {}) });

/** The thread at work since 10. */
const working = { r1: request("r1", 10, { type: "working" }) };
const board = (parts: Partial<BackgroundEvents>): BackgroundEvents =>
  ({ orchestratorSteps: [], requests: working, ...parts });
/** The event rows as they read: name, state, more words, button. */
const events = (parts: Partial<BackgroundEvents>) =>
  backgroundRows([], null, () => null, board(parts)).map((row) => row.type === "event" ? [row.name, row.state, row.words, row.action?.type ?? null] : [row.id]);

test("the CI watch is a row with Open CI while it watches, and leaves when its result comes", () => {
  const watching = backgroundRows([], null, () => null, board({ orchestratorSteps: [ci(30, [ciRun("in_progress", "", 3, 5)])] }));
  assert.deepEqual(watching.map((row) => row.type === "event" && [row.name, row.state, row.words, row.action]), [["CI", "checking", "3 of 5 jobs", { type: "openCi", url: "https://ci/test" }]]);
  assert.deepEqual(events({ orchestratorSteps: [ci(30, [])] }), [["CI", "checking", null, null]]);
  assert.deepEqual(events({ orchestratorSteps: [ci(30, [ciRun("in_progress", "", 3, 5)]), ci(40, [ciRun("completed", "failure", 5, 5)], "failed")] }), []);
  assert.deepEqual(events({ orchestratorSteps: [ci(30, [ciRun("completed", "success", 5, 5)], "passed")] }), []);
});

test("a merge at work is a row of its own, and leaves once over", () => {
  const requested = step(60, { type: "mergeRequested", id: "m", branch: "s", base: "main", origin: {} });
  assert.deepEqual(events({ orchestratorSteps: [requested] }), [["Merge into main", "merging", null, null]]);
  const merged = step(80, { type: "merged", branch: "s", base: "main", commits: 3, askedIn: "r1", head: "abc", mergeCommit: "def" });
  assert.deepEqual(events({ orchestratorSteps: [requested, merged] }), []);
  // A click whose work is over (turned down, or stopped) merges nothing.
  assert.deepEqual(events({ orchestratorSteps: [requested], requests: { r1: request("r1", 10, { type: "done" }) } }), []);
});

test("event rows sort newest first with the processes", () => {
  const sorted = backgroundRows([preview("preview-1", "Dev app", "pnpm dev", 25), preview("preview-2", "Docs", "pnpm docs", 20)], null, () => null,
    board({ orchestratorSteps: [ci(100, [])] }));
  assert.deepEqual(sorted.map((row) => row.id), ["ci", "preview-1", "preview-2"]);
});
