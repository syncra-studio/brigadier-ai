import assert from "node:assert/strict";
import { test } from "node:test";

import { stripWords, workersStrip } from "@/app/conversation/activity/stripView";
import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import type { Task } from "@/ipc/generated";

const base = Object.values(night.tasks)[0] as unknown as Task;
let next = 0;
const task = (patch: Partial<Task> = {}): Task => {
  next += 1;
  return { ...base, id: `t${next}`, number: next, state: "running", createdAtMs: next * 10, updatedAtMs: next * 10, ...patch };
};

test("the strip lists only the workers running or waiting, oldest first; finished ones are the context card's", () => {
  const running = task();
  const queued = task({ state: "queued" });
  const finished = [task({ state: "done" }), task({ state: "stopped" }), task({ state: "failed" }), task({ state: "landed" })];
  const view = workersStrip([queued, ...finished, running]);
  assert.deepEqual(view?.rows, [running.id, queued.id]);
  assert.equal(view?.working, 1);
  assert.equal(view?.waiting, 1);
  assert.equal(stripWords(view!), "1 worker working · 1 waiting");
});

test("with no worker at work the strip is gone", () => {
  assert.equal(workersStrip([]), null);
  assert.equal(workersStrip([task({ state: "done" }), task({ state: "stopped" }), task({ state: "failed" })]), null);
});

test("Stop all shows only while a worker runs or waits to run", () => {
  const landing = task({ state: "landing" });
  assert.equal(workersStrip([landing, task({ state: "queued" })])?.stoppable, true);
  assert.equal(workersStrip([task({ state: "running" })])?.stoppable, true);
});

test("the collapsed strip counts in one line, naming workers once", () => {
  assert.equal(stripWords({ working: 2, waiting: 0 }), "2 workers working");
  assert.equal(stripWords({ working: 1, waiting: 1 }), "1 worker working · 1 waiting");
  assert.equal(stripWords({ working: 0, waiting: 3 }), "3 workers waiting");
});
