import assert from "node:assert/strict";
import { test } from "node:test";

import { appCommand, backgroundRows } from "@/app/conversation/backgroundRows";
import type { Preview, PreviewState } from "@/ipc/generated";

const preview = (id: string, name: string, command: string, startedAtMs: number, state: PreviewState = { type: "running" }, extra: Partial<Preview> = {}): Preview =>
  ({ id, conversationId: "c", name, command, workdir: "/w", workspace: "/w", pid: 1, state, startedAtMs, endedAtMs: null, log: null, kind: "web", building: false, ...extra }) as Preview;

const rows = (previews: Preview[]) =>
  backgroundRows(previews, () => null).map((row) => [row.type === "preview" ? row.preview.name : row.id, row.state]);

const exited = (code: number | null) => ({ type: "exited", code, status: code === null ? "killed by signal 9" : `exit ${code}` }) as PreviewState;

test("a worker's smoke test of the running app is a state of the app's row, not a row of its own", () => {
  const app = preview("preview-4", "todo app", 'PORT=3000 TODO_FILE="$TMPDIR/todos.json" node src/server.js', 10);
  const smoke = preview("preview-5", "todo smoke", 'PORT=38471 TODO_FILE="/scratch/t.json" node src/server.js', 20, { type: "running" }, { taskId: "t3" });
  assert.deepEqual(rows([app]), [["todo app", "running"]]);
  assert.deepEqual(rows([app, smoke]), [["todo app", "smoke testing"]]);
  // Any other worker's copy checks against it; a browser at it smoke-tests it.
  assert.deepEqual(rows([app, { ...smoke, name: "todo server" }]), [["todo app", "checking"]]);
  const browser = preview("preview-6", "Todo browser", "chrome-headless-shell --remote-debugging-port=43987 http://localhost:3000/", 30);
  assert.deepEqual(rows([app, browser]), [["todo app", "smoke testing"]]);
  // A copy that ended leaves the app running again.
  assert.deepEqual(rows([app, { ...smoke, state: { type: "stopped", reason: "stopped by its worker" } }]), [["todo app", "running"]]);
});

test("a worker's preview row names the worker from its rows", () => {
  const own = preview("preview-7", "API", "node api.js", 10, { type: "running" }, { taskId: "w1" });
  const [row] = backgroundRows([own], (taskId) => (taskId === "w1" ? "clamp util" : null));
  assert.equal(row?.type === "preview" && row.worker, "clamp util");
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

test("rows sort newest first", () => {
  const sorted = backgroundRows([preview("preview-1", "Dev app", "pnpm dev", 25), preview("preview-2", "Docs", "pnpm docs", 20)], () => null);
  assert.deepEqual(sorted.map((row) => row.id), ["preview-1", "preview-2"]);
});
