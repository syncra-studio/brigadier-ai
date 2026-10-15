import assert from "node:assert/strict";
import { test } from "node:test";

import { outcomeWords, workerOutcome } from "@/app/conversation/workerPresentation";
import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import type { Task } from "@/ipc/generated";

const base = Object.values(night.tasks)[0] as unknown as Task;
const task = (state: Task["state"], landed: string | null = null): Task => ({ ...base, state, landed });

test("only work that landed or was used counts as done; a worker stopped before it landed reads stopped", () => {
  assert.equal(workerOutcome(task("running")), null);
  assert.equal(workerOutcome(task("reported")), "done");
  assert.equal(workerOutcome(task("landed", "abc1234")), "done");
  assert.equal(workerOutcome(task("done")), "done");
  assert.equal(workerOutcome(task("stopped")), "stopped");
  assert.equal(workerOutcome(task("rejected")), "stopped");
  // Stopped after its work landed: what it made is in.
  assert.equal(workerOutcome(task("stopped", "abc1234")), "done");
  assert.equal(workerOutcome(task("failed")), "failed");
  assert.equal(outcomeWords([task("done"), task("stopped"), task("running")]), "1 done · 1 stopped");
  assert.equal(outcomeWords([task("stopped"), task("failed")]), "1 stopped · 1 failed");
  assert.equal(outcomeWords([task("running")]), "");
});
