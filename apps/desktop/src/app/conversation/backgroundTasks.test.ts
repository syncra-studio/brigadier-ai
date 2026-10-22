import assert from "node:assert/strict";
import { test } from "node:test";

import { AGENT_STAYS_MS, SHELL_STAYS_MS, taskShown } from "@/app/conversation/backgroundTasks";
import type { BackgroundTask } from "@/ipc/generated";

const task = (kind: BackgroundTask["kind"], status: BackgroundTask["status"], endedAtMs: number | null = null): BackgroundTask => ({
  id: "t", itemId: null, kind, label: "t", status, startedAtMs: 0, endedAtMs, outputFile: null, summary: null, lastLine: null,
});

test("a background task keeps its row by the CLI's own rules", () => {
  const now = 1_000_000;
  for (const kind of ["shell", "terminal", "agent", "other"] as const) {
    assert.equal(taskShown(task(kind, "running"), now), true, `a running ${kind} shows`);
    assert.equal(taskShown(task(kind, "failed", now), now), false, `a failed ${kind} leaves`);
    assert.equal(taskShown(task(kind, "stopped", now), now), false, `a stopped ${kind} leaves`);
  }
  assert.equal(taskShown(task("agent", "completed", now - AGENT_STAYS_MS + 1), now), true);
  assert.equal(taskShown(task("agent", "completed", now - AGENT_STAYS_MS), now), false);
  assert.equal(taskShown(task("other", "completed", now - AGENT_STAYS_MS), now), false);
  assert.equal(taskShown(task("shell", "completed", now - SHELL_STAYS_MS + 1), now), true);
  assert.equal(taskShown(task("shell", "completed", now - SHELL_STAYS_MS), now), false);
  assert.equal(taskShown(task("terminal", "completed", now), now), false, "a Codex terminal leaves when Codex stops listing it");
});
