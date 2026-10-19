import assert from "node:assert/strict";
import { test } from "node:test";

import { appCommand, backgroundRows, runState } from "@/app/conversation/backgroundRows";
import type { BackgroundRun, Preview, PreviewState, Task } from "@/ipc/generated";

const preview = (id: string, name: string, command: string, startedAtMs: number, state: PreviewState = { type: "running" }, extra: Partial<Preview> = {}): Preview =>
  ({ id, conversationId: "c", name, command, workdir: "/w", workspace: "/w", pid: 1, state, startedAtMs, endedAtMs: null, log: null, kind: "web", building: false, ...extra }) as Preview;

const run = (id: string, command: string, startedAtMs: number): BackgroundRun =>
  ({ id, conversationId: "c", requestId: null, tool: "Bash", command, startedAtMs, endedAtMs: null, state: { type: "running" } }) as BackgroundRun;

const rows = (previews: Preview[], runs: BackgroundRun[] = [], tasks: Record<string, Task> = {}) =>
  backgroundRows(previews, runs, tasks, () => null).map((row) => [row.type === "preview" ? row.preview.name : row.run.command, row.state]);

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
