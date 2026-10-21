import assert from "node:assert/strict";
import { test } from "node:test";

import { appCommand, type BackgroundLeft, backgroundRows, runState } from "@/app/conversation/backgroundRows";
import type { BackgroundRun, Preview, PreviewState, Task } from "@/ipc/generated";

const preview = (id: string, name: string, command: string, startedAtMs: number, state: PreviewState = { type: "running" }, extra: Partial<Preview> = {}): Preview =>
  ({ id, conversationId: "c", name, command, workdir: "/w", workspace: "/w", pid: 1, state, startedAtMs, endedAtMs: null, log: null, kind: "web", building: false, ...extra }) as Preview;

const run = (id: string, command: string, startedAtMs: number): BackgroundRun =>
  ({ id, conversationId: "c", requestId: null, tool: "Bash", command, startedAtMs, endedAtMs: null, state: { type: "running" } }) as BackgroundRun;

const rows = (previews: Preview[], runs: BackgroundRun[] = [], tasks: Record<string, Task> = {}, left?: BackgroundLeft) =>
  backgroundRows(previews, runs, tasks, () => null, left).map((row) => [row.type === "preview" ? row.preview.name : row.run.command, row.state]);

const ended = (record: BackgroundRun, state: BackgroundRun["state"], endedAtMs: number): BackgroundRun => ({ ...record, state, endedAtMs });
const exited = (code: number | null) => ({ type: "exited", code, status: code === null ? "killed by signal 9" : `exit ${code}` }) as PreviewState;

test("a worker's smoke test of the running app is a state of the app's row, not a row of its own", () => {
  const app = preview("preview-4", "todo app", 'PORT=3000 TODO_FILE="$TMPDIR/todos.json" node src/server.js', 10);
  const smoke = preview("preview-5", "todo smoke", 'PORT=38471 TODO_FILE="/scratch/t.json" node src/server.js', 20, { type: "running" }, { taskId: "t3" });
  const look = { t3: { id: "t3", liveLook: true } as Task };
  assert.deepEqual(rows([app]), [["todo app", "running"]]);
  assert.deepEqual(rows([app, smoke], [], look), [["todo app", "smoke testing"]]);
  // Any other worker's copy checks against it; a browser at it smoke-tests it.
  assert.deepEqual(rows([app, { ...smoke, name: "todo server" }]), [["todo app", "checking"]]);
  const browser = preview("preview-6", "Todo browser smoke", "chrome-headless-shell --remote-debugging-port=43987 http://localhost:3000/", 30);
  assert.deepEqual(rows([app, browser]), [["todo app", "smoke testing"]]);
  // A copy that ended leaves the app running again.
  assert.deepEqual(rows([app, { ...smoke, state: { type: "stopped", reason: "stopped by its worker" } }], [], look), [["todo app", "running"]]);
});

test("a row's state follows its process: building, starting, running, paused; a command's from what it runs", () => {
  assert.deepEqual(rows([preview("p", "Dev app", "pnpm tauri dev", 1, { type: "starting" }, { building: true })]), [["Dev app", "building"]]);
  assert.deepEqual(rows([preview("p", "Dev app", "pnpm dev", 1, { type: "starting" })]), [["Dev app", "starting"]]);
  assert.deepEqual(rows([preview("p", "Dev app", "pnpm dev", 1, { type: "paused" })]), [["Dev app", "paused"]]);
  assert.deepEqual(rows([], [run("r1", "cargo build --release", 1), run("r2", "pnpm test", 2), run("r3", "python3 -m http.server", 3)]), [
    ["python3 -m http.server", "running"],
    ["pnpm test", "checking"],
    ["cargo build --release", "building"],
  ]);
  assert.equal(runState("npm run build"), "building");
  assert.equal(appCommand("PORT=1 A='b c' node x.js"), "node x.js");
});

test("a row stays while it still needs the user (Q94): failed until dealt with, finished or stopped never", () => {
  const crashed = preview("preview-1", "Dev app", "pnpm dev", 10, exited(1), { endedAtMs: 50 });
  const done = preview("preview-2", "Docs build", "pnpm docs", 20, exited(0), { endedAtMs: 60 });
  const stopped = preview("preview-3", "Web app", "pnpm web", 30, { type: "stopped", reason: "stopped by the user" }, { endedAtMs: 70 });
  const paused = preview("preview-4", "API", "node api.js", 40, { type: "paused" });
  // Failed: a non-zero exit, a crash; finished (a clean exit) or stopped by the user: gone; paused: stays.
  assert.deepEqual(rows([crashed, done, stopped, paused]), [["API", "paused"], ["Dev app", "failed"]]);
  assert.deepEqual(rows([{ ...crashed, state: exited(null) }]), [["Dev app", "failed"]]);
  // Dismissed (or restarted, for a run): it leaves.
  assert.deepEqual(rows([crashed, done], [], {}, { cleared: ["preview-1"] }), []);
  // Restarted under its id, it is the running row again.
  assert.deepEqual(rows([{ ...crashed, state: { type: "starting" } }]), [["Dev app", "starting"]]);
  // The thread starts the same process again: the new one is the row, running or ended.
  const again = preview("preview-5", "Dev app", "PORT=4000 pnpm dev", 90);
  assert.deepEqual(rows([crashed, again]), [["Dev app", "running"]]);
  assert.deepEqual(rows([crashed, { ...again, state: { type: "stopped", reason: "stopped by the user" } }]), []);
  // A worker's process that ended is its worker's: no row.
  assert.deepEqual(rows([{ ...crashed, taskId: "t1" }]), []);
});

test("a background run's row: a non-zero exit or an interruption failed; exit 0 or a Stop leaves", () => {
  const tests = run("run-1", "pnpm test", 10);
  const build = run("run-2", "cargo build", 20);
  assert.deepEqual(rows([], [ended(tests, { type: "finished", status: "exit 1" }, 30), ended(build, { type: "finished", status: "exit 0" }, 40)]), [
    ["pnpm test", "failed"],
  ]);
  assert.deepEqual(rows([], [ended(tests, { type: "interrupted", reason: "Brigadier restarted" }, 30)]), [["pnpm test", "failed"]]);
  assert.deepEqual(rows([], [ended(tests, { type: "finished", status: "timed out after 600 s" }, 30)]), [["pnpm test", "failed"]]);
  assert.deepEqual(rows([], [ended(tests, { type: "stopped" }, 30)]), []);
  // Restart or dismiss (or a later pass of its command), and the thread's next run of it.
  assert.deepEqual(rows([], [ended(tests, { type: "finished", status: "exit 1" }, 30)], {}, { cleared: ["run-1"] }), []);
  assert.deepEqual(rows([], [ended(tests, { type: "finished", status: "exit 1" }, 30), run("run-3", "pnpm test", 50)]), [["pnpm test", "checking"]]);
});
