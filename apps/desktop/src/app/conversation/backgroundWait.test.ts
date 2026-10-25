import assert from "node:assert/strict";
import { test } from "node:test";

import { taskRows, workerTaskRows } from "@/app/conversation/backgroundRows";
import type { BackgroundTask, FileEntry, WorkerRow } from "@/ipc/generated";

const task = (kind: BackgroundTask["kind"], label: string, extra: Partial<BackgroundTask> = {}): BackgroundTask => ({
  id: "t", itemId: "bash-1", kind, label, status: "running", startedAtMs: 10, endedAtMs: null,
  outputFile: null, summary: null, lastLine: null, ...extra,
});

/** A Bash call as Claude's thread holds it: a `command` item (D38). */
const claude = (command: string, extra: Partial<FileEntry> = {}, itemId = "bash-1"): FileEntry => ({
  event: { type: "command", itemId, command, cwd: null, status: "completed", exitCode: null, output: null, durationMs: null },
  ...extra,
});

/** A tool call with a `command` argument. */
const bash = (input: unknown, extra: Partial<FileEntry> = {}, itemId = "bash-1"): FileEntry => ({
  event: { type: "toolCall", itemId, name: "Bash", input: JSON.stringify(input), status: "completed", output: null },
  ...extra,
});

test("Claude waits use the matching call's command, not the model-written task label", () => {
  const wait = task("shell", "Wait for the workers to finish");
  const entries = [
    claude("pnpm test", {}, "unrelated"),
    claude("brigadier wait w1"),
  ];
  assert.deepEqual(taskRows([wait], entries, 0), []);
  // A misleading label never hides an ordinary command.
  const ordinary = task("shell", "brigadier wait");
  assert.equal(taskRows([ordinary], [claude("pnpm test")], 0).length, 1);
  assert.equal(taskRows([ordinary], [claude("brigadier wait", {}, "unrelated"), claude("pnpm test")], 0).length, 1);
  assert.equal(taskRows([ordinary], [claude("echo 'brigadier wait'")], 0).length, 1);
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
    assert.deepEqual(taskRows([task("terminal", command)], [], 0), [], `Codex: ${command}`);
    assert.deepEqual(taskRows([task("shell", "Workers")], [claude(command)], 0), [], `Claude: ${command}`);
  }
});

test("other commands and quoted mentions of waits keep their rows", () => {
  for (const command of [
    "pnpm test", "sleep 60", "wait", "brigadier start fix", "brigadier preview web", "brigadier waiting",
    "other-brigadier wait", "echo brigadier wait", "echo 'brigadier start fix && brigadier wait'",
    'printf "%s" "brigadier wait"', "rg 'brigadier wait' src", "brigadier wait-for-worker",
  ]) {
    assert.equal(taskRows([task("terminal", command)], [], 0).length, 1, `Codex: ${command}`);
    assert.equal(taskRows([task("shell", "Wait for workers")], [claude(command)], 0).length, 1, `Claude: ${command}`);
  }
});

test("the recorded wait of session 698142a4 has no row (D38)", () => {
  // Claude's task_started and its Bash call, as the thread got them.
  const wait = task("shell", "Wait for the next worker event", { id: "b8ea764yr", itemId: "toolu_01Wrv7Adp1K2MrmL7UZnMh66" });
  const pushChecks = task("shell", "Pre-push checks for composer", { id: "b2", itemId: "toolu_checks" });
  const entries = [
    claude("brigadier wait", {}, "toolu_01Wrv7Adp1K2MrmL7UZnMh66"),
    claude("pnpm -C apps/desktop typecheck && cargo clippy", {}, "toolu_checks"),
  ];
  assert.deepEqual(taskRows([wait, pushChecks], entries, 0).map((row) => row.id), ["task:b2"]);
});

test("a tool call's command argument is read too", () => {
  assert.deepEqual(taskRows([task("shell", "Workers")], [bash({ command: "brigadier wait", run_in_background: true })], 0), []);
});

test("a missing or unreadable Claude command keeps the task, without using its label", () => {
  const waitLabel = task("shell", "brigadier wait");
  for (const entries of [
    [], [bash(null)], [bash({ command: 42 })], [bash({ description: "brigadier wait" })],
    [bash({ command: "brigadier wait" }, { parent: "child-agent" })],
    [{ event: { type: "toolCall", itemId: "bash-1", name: "Bash", input: '{"command":', status: "inProgress", output: null } } as FileEntry],
  ]) {
    assert.equal(taskRows([waitLabel], entries, 0).length, 1);
  }
  assert.equal(taskRows([{ ...waitLabel, itemId: null }], [bash({ command: "brigadier wait" })], 0).length, 1);
  for (const kind of ["agent", "other"] as const) {
    assert.equal(taskRows([task(kind, "brigadier wait")], [bash({ command: "brigadier wait" })], 0).length, 1);
  }
});

test("task rows say how a task ended for about 5 min, then drop it, and leave the task history intact (D21)", () => {
  const tasks = [
    task("shell", "Tests"),
    ...(["completed", "failed", "stopped"] as const).map((status) => task("shell", "Build", { id: status, status, endedAtMs: 20 })),
  ];
  const before = structuredClone(tasks);
  assert.deepEqual(taskRows(tasks, [], 30).map((row) => [row.id, row.state]),
    [["task:t", "running"], ["task:completed", "finished"], ["task:failed", "failed"], ["task:stopped", "stopped"]]);
  assert.deepEqual(taskRows(tasks, [], 20 + 5 * 60_000).map((row) => [row.id, row.state]), [["task:t", "running"]]);
  assert.deepEqual(tasks, before);
});

const worker = (background: WorkerRow["background"]) => ({ id: "w1", background }) as WorkerRow;

test("a worker's background commands are rows under its name, by the session's rules", () => {
  const rows = workerTaskRows([worker([
    { task: task("shell", "Run the dev app", { id: "dev" }), command: "pnpm tauri:dev" },
    { task: task("shell", "Wait for my helper", { id: "wait" }), command: "brigadier wait w3" },
    { task: task("terminal", "brigadier wait", { id: "codex-wait" }), command: null },
    { task: task("agent", "Look around", { id: "agent" }), command: null },
    { task: task("shell", "Old build", { id: "done", status: "completed", endedAtMs: 20 }), command: "pnpm build" },
  ])], () => "Rail fix", 20 + 5 * 60_000);
  assert.deepEqual(rows.map((row) => [row.id, row.type === "task" && row.worker]), [["task:w1:dev", { id: "w1", name: "Rail fix" }]]);
  // Ended a moment ago, it still says how.
  const recent = workerTaskRows([worker([
    { task: task("shell", "Old build", { id: "done", status: "completed", endedAtMs: 20, summary: 'Background command "pnpm build" completed (exit code 0)' }), command: "pnpm build" },
    { task: task("shell", "Wait", { id: "wait", status: "completed", endedAtMs: 20 }), command: "brigadier wait w3" },
  ])], () => "Rail fix", 30);
  assert.deepEqual(recent.map((row) => [row.id, row.state]), [["task:w1:done", "finished"]]);
  assert.deepEqual(workerTaskRows([{ id: "w2" } as WorkerRow], () => null, 0), []);
});
