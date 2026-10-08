import assert from "node:assert/strict";
import { test } from "node:test";

import { stripWords, workersStrip } from "@/app/conversation/activity/stripView";
import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import type { Task, UserRequest, WorkerStep } from "@/ipc/generated";

const base = Object.values(night.tasks)[0] as unknown as Task;
let next = 0;
const task = (patch: Partial<Task> = {}): Task => {
  next += 1;
  return { ...base, id: `t${next}`, number: next, state: "running", createdAtMs: next * 10, updatedAtMs: next * 10, ...patch };
};
const sent = (startedAtMs: number) => ({ startedAtMs }) as UserRequest;

test("the strip is about workers at it and those finished since the user's last message", () => {
  const older = task({ state: "done", updatedAtMs: 5 });
  const running = task();
  const waiting = task({ state: "paused" });
  const finished = task({ state: "reported", updatedAtMs: 500 });
  const view = workersStrip([finished, waiting, older, running], [sent(1), sent(100)], []);
  assert.deepEqual(view, { rows: [running.id, waiting.id, finished.id], working: 1, waiting: 1, done: 1, stoppable: true });
  // A new message: what finished before it leaves the strip; nothing left hides it.
  assert.equal(workersStrip([older, finished], [sent(1_000)], []), null);
  assert.equal(workersStrip([], [], []), null);
});

const ended = (taskId: string, atMs: number, kind: WorkerStep["kind"] = "stopped"): WorkerStep =>
  ({ taskId, requestId: null, kind, atMs, position: 0 });

test("a worker counts as finished when it ended, not when it was last updated", () => {
  // Stopped before the user's last message, its kept patch restored after it: still not new.
  const restored = task({ state: "stopped", updatedAtMs: 900 });
  assert.equal(workersStrip([restored], [sent(500)], [ended(restored.id, 100), ended(restored.id, 950, "updated")]), null);
  assert.equal(workersStrip([restored], [sent(500)], [ended(restored.id, 600)])?.done, 1);
});

test("Stop all shows only while a worker runs or waits to run", () => {
  const landing = task({ state: "landing" });
  const ready = task({ state: "readyToLand" });
  assert.equal(workersStrip([landing, ready], [], [])?.stoppable, false);
  assert.equal(workersStrip([landing, task({ state: "queued" })], [], [])?.stoppable, true);
});

test("the collapsed strip counts in one line, naming workers once", () => {
  assert.equal(stripWords({ working: 2, waiting: 0, done: 1 }), "2 workers working · 1 done");
  assert.equal(stripWords({ working: 1, waiting: 1, done: 0 }), "1 worker working · 1 waiting");
  assert.equal(stripWords({ working: 0, waiting: 0, done: 3 }), "3 workers done");
});
