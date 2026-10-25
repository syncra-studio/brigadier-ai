import assert from "node:assert/strict";
import { test } from "node:test";

import { taskShown } from "@/app/conversation/backgroundTasks";
import type { BackgroundTask } from "@/ipc/generated";

const task = (kind: BackgroundTask["kind"], status: BackgroundTask["status"], endedAtMs: number | null = null): BackgroundTask => ({
  id: "t", itemId: null, kind, label: "t", status, startedAtMs: 0, endedAtMs, outputFile: null, summary: null, lastLine: null,
});

test("only running tasks keep a row; completed, failed and stopped tasks leave immediately", () => {
  for (const kind of ["shell", "terminal", "agent", "other"] as const) {
    assert.equal(taskShown(task(kind, "running")), true, `a running ${kind} shows`);
    for (const status of ["completed", "failed", "stopped"] as const) {
      for (const endedAtMs of [null, 0, 1_000_000]) {
        assert.equal(taskShown(task(kind, status, endedAtMs)), false, `a ${status} ${kind} leaves, even with endedAtMs=${endedAtMs}`);
      }
    }
  }
});
