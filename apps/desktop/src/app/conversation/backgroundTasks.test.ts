import assert from "node:assert/strict";
import { test } from "node:test";

import { AGENT_STAYS_MS, COMMAND_STAYS_MS, endWords, taskEnd, taskShown } from "@/app/conversation/backgroundTasks";
import { taskRows } from "@/app/conversation/backgroundRows";
import type { BackgroundTask } from "@/ipc/generated";

const task = (kind: BackgroundTask["kind"], status: BackgroundTask["status"], endedAtMs: number | null = null, summary: string | null = null): BackgroundTask => ({
  id: "t", itemId: null, kind, label: "t", status, startedAtMs: 0, endedAtMs, outputFile: null, summary, lastLine: null,
});

test("a background task keeps its row by the CLI's own rules: a command that ended about 5 min, a subagent 30 s (D21)", () => {
  const now = 1_000_000;
  for (const kind of ["shell", "terminal", "agent", "other"] as const) {
    assert.equal(taskShown(task(kind, "running"), now), true, `a running ${kind} shows`);
  }
  for (const kind of ["shell", "terminal", "other"] as const) {
    for (const status of ["completed", "failed", "stopped"] as const) {
      assert.equal(taskShown(task(kind, status, now - COMMAND_STAYS_MS + 1), now), true, `a ${status} ${kind} stays`);
      assert.equal(taskShown(task(kind, status, now - COMMAND_STAYS_MS), now), false, `a ${status} ${kind} goes after 5 min`);
    }
  }
  assert.equal(taskShown(task("agent", "completed", now - AGENT_STAYS_MS + 1), now), true);
  assert.equal(taskShown(task("agent", "completed", now - AGENT_STAYS_MS), now), false);
});

const words = (status: BackgroundTask["status"], summary: string | null) => {
  const end = taskEnd(task("shell", status, 1, summary));
  return end && endWords(end);
};

test("a task that ended says how, with its exit code when the CLI said one", () => {
  assert.equal(words("running", null), null);
  assert.equal(words("completed", 'Background command "pnpm test" completed (exit code 0)'), "finished · exit 0");
  assert.equal(words("completed", 'Background command "pnpm test" completed (exit code 1)'), "failed · exit 1");
  assert.equal(words("failed", 'Background command "pnpm test" failed with exit code 2'), "failed · exit 2");
  assert.equal(words("stopped", 'Background command "pnpm test" was stopped'), "stopped");
  // A Codex terminal Codex stopped listing: no code.
  assert.equal(words("completed", null), "finished");
});

test("the card's rows of the CLI's tasks: running, then how each ended for about 5 min", () => {
  const now = 1_000_000;
  const tasks = [
    { ...task("shell", "running"), id: "a" },
    { ...task("shell", "completed", now - 1000, 'Background command "x" completed (exit code 0)'), id: "b" },
    { ...task("terminal", "completed", now - 1000), id: "c" },
    { ...task("shell", "stopped", now - COMMAND_STAYS_MS), id: "d" },
  ];
  assert.deepEqual(taskRows(tasks, [], now).map((row) => [row.id, row.state]), [["task:a", "running"], ["task:b", "finished"], ["task:c", "finished"]]);
});
