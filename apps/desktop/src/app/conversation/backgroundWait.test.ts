import assert from "node:assert/strict";
import { test } from "node:test";

import { taskRows, workerTaskRows } from "@/app/conversation/backgroundRows";
import type { BackgroundTask, FileEntry, WorkerRow } from "@/ipc/generated";

const task = (kind: BackgroundTask["kind"], label: string, extra: Partial<BackgroundTask> = {}): BackgroundTask => ({
  id: "t", itemId: "bash-1", kind, label, status: "running", startedAtMs: 10, endedAtMs: null,
  outputFile: null, summary: null, lastLine: null, ...extra,
});

const bash = (input: unknown, extra: Partial<FileEntry> = {}, itemId = "bash-1"): FileEntry => ({
  event: { type: "toolCall", itemId, name: "Bash", input: JSON.stringify(input), status: "completed", output: null },
  ...extra,
});

test("Claude waits use the matching tool call's command, not the model-written task label", () => {
  const wait = task("shell", "Wait for the workers to finish");
  const entries = [
    bash({ command: "pnpm test" }, {}, "unrelated"),
    bash({ command: "brigadier wait w1", run_in_background: true }),
  ];
  assert.deepEqual(taskRows([wait], entries), []);
  // A misleading label never hides an ordinary command.
  const ordinary = task("shell", "brigadier wait");
  assert.equal(taskRows([ordinary], [bash({ command: "pnpm test", run_in_background: true })]).length, 1);
  assert.equal(taskRows([ordinary], [bash({ command: "brigadier wait" }, {}, "unrelated"), bash({ command: "pnpm test" })]).length, 1);
  assert.equal(taskRows([ordinary], [bash({ command: "echo 'brigadier wait'" })]).length, 1);
});

test("direct, chained and shell-wrapped waits have no row for either CLI", () => {
  for (const command of [
    "brigadier wait",
    "brigadier wait w1 w2 --json",
    'brigadier start "implement the fix" && brigadier wait',
    "brigadier start fix; brigadier wait w1",
    "brigadier start fix\nbrigadier wait",
    "brigadier start fix || brigadier wait",
    "BRIGADIER_SESSION_ID=s brigadier wait",
    "/usr/local/bin/brigadier wait",
    "/bin/zsh -lc 'cd /repo && brigadier start fix && brigadier wait'",
  ]) {
    assert.deepEqual(taskRows([task("terminal", command)], []), [], `Codex: ${command}`);
    assert.deepEqual(taskRows([task("shell", "Workers")], [bash({ command, run_in_background: true })]), [], `Claude: ${command}`);
  }
});

test("other commands and quoted mentions of waits keep their rows", () => {
  for (const command of [
    "pnpm test", "sleep 60", "wait", "brigadier start fix", "brigadier preview web", "brigadier waiting",
    "other-brigadier wait", "echo brigadier wait", "echo 'brigadier start fix && brigadier wait'",
    'printf "%s" "brigadier wait"', "rg 'brigadier wait' src", "brigadier wait-for-worker",
  ]) {
    assert.equal(taskRows([task("terminal", command)], []).length, 1, `Codex: ${command}`);
    assert.equal(taskRows([task("shell", "Wait for workers")], [bash({ command })]).length, 1, `Claude: ${command}`);
  }
});

test("a missing or unreadable Claude command keeps the task, without using its label", () => {
  const waitLabel = task("shell", "brigadier wait");
  for (const entries of [
    [], [bash(null)], [bash({ command: 42 })], [bash({ description: "brigadier wait" })],
    [bash({ command: "brigadier wait" }, { parent: "child-agent" })],
    [{ event: { type: "toolCall", itemId: "bash-1", name: "Bash", input: '{"command":', status: "inProgress", output: null } } as FileEntry],
  ]) {
    assert.equal(taskRows([waitLabel], entries).length, 1);
  }
  assert.equal(taskRows([{ ...waitLabel, itemId: null }], [bash({ command: "brigadier wait" })]).length, 1);
  for (const kind of ["agent", "other"] as const) {
    assert.equal(taskRows([task(kind, "brigadier wait")], [bash({ command: "brigadier wait" })]).length, 1);
  }
});

test("task rows drop finished tasks immediately and leave the task history intact", () => {
  const tasks = [
    task("shell", "Tests"),
    ...(["completed", "failed", "stopped"] as const).map((status) => task("shell", "Build", { id: status, status })),
  ];
  const before = structuredClone(tasks);
  assert.deepEqual(taskRows(tasks, []).map((row) => [row.id, row.state]), [["task:t", "running"]]);
  assert.deepEqual(tasks, before);
});

const worker = (background: WorkerRow["background"]) => ({ id: "w1", background }) as WorkerRow;

test("a worker's running background commands are rows under its name, by the session's rules", () => {
  const rows = workerTaskRows([worker([
    { task: task("shell", "Run the dev app", { id: "dev" }), command: "pnpm tauri:dev" },
    { task: task("shell", "Wait for my helper", { id: "wait" }), command: "brigadier wait w3" },
    { task: task("terminal", "brigadier wait", { id: "codex-wait" }), command: null },
    { task: task("agent", "Look around", { id: "agent" }), command: null },
    { task: task("shell", "Old build", { id: "done", status: "completed", endedAtMs: 20 }), command: "pnpm build" },
  ])], () => "Rail fix");
  assert.deepEqual(rows.map((row) => [row.id, row.type === "task" && row.worker]), [["task:w1:dev", { id: "w1", name: "Rail fix" }]]);
  assert.deepEqual(workerTaskRows([{ id: "w2" } as WorkerRow], () => null), []);
});
