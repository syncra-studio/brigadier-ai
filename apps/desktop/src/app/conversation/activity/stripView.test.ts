import assert from "node:assert/strict";
import { test } from "node:test";

import { lastReplies, stripWords, workersStrip } from "@/app/conversation/activity/stripView";
import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import type { Message, Task, WorkerStep } from "@/ipc/generated";

const base = Object.values(night.tasks)[0] as unknown as Task;
let next = 0;
const task = (patch: Partial<Task> = {}): Task => {
  next += 1;
  return { ...base, id: `t${next}`, number: next, state: "running", createdAtMs: next * 10, updatedAtMs: next * 10, ...patch };
};
const ended = (taskId: string, atMs: number, kind: WorkerStep["kind"] = "stopped"): WorkerStep =>
  ({ taskId, requestId: null, kind, atMs, position: 0 });
const reply = (requestId: string | null, createdAtMs: number, role: Message["role"] = "assistant") =>
  ({ role, requestId, createdAtMs }) as Message;

test("a finished worker stays until its request's answer is posted, not before", () => {
  const finished = task({ state: "reported", requestId: "r1", updatedAtMs: 500 });
  const running = task({ requestId: "r2" });
  // A reply before it finished (the thread's "Starting 2 workers") isn't the answer.
  const before = lastReplies([reply("r1", 100), reply("r1", 600, "user")]);
  assert.deepEqual(workersStrip([finished, running], before, [])?.rows, [running.id, finished.id]);
  // The answer for its request, after it finished, takes it off; the live one stays.
  const answered = lastReplies([reply("r1", 100), reply("r1", 700)]);
  assert.deepEqual(workersStrip([finished, running], answered, []), {
    rows: [running.id], working: 1, waiting: 0, done: 0, stopped: 0, failed: 0, stoppable: true,
  });
  // Another request's answer doesn't.
  assert.equal(workersStrip([finished], lastReplies([reply("r2", 900)]), [])?.done, 1);
  assert.equal(workersStrip([finished], answered, []), null);
  assert.equal(workersStrip([], {}, []), null);
});

test("a live worker always shows, and keeps its request's finished workers in view", () => {
  const first = task({ state: "done", requestId: "r1", updatedAtMs: 200 });
  const second = task({ state: "paused", requestId: "r1" });
  // A progress line after the first landed is no answer while the second still works.
  const view = workersStrip([first, second], lastReplies([reply("r1", 300)]), []);
  assert.deepEqual(view, { rows: [second.id, first.id], working: 0, waiting: 1, done: 1, stopped: 0, failed: 0, stoppable: true });
  // A worker with no request has no answer to wait for.
  assert.equal(workersStrip([task({ state: "done", requestId: null })], {}, []), null);
  assert.equal(workersStrip([task({ requestId: null })], {}, [])?.working, 1);
});

test("a worker counts as finished when it ended, not when it was last updated", () => {
  // Stopped before the answer, its kept patch restored after it: still answered.
  const restored = task({ state: "stopped", requestId: "r1", updatedAtMs: 900 });
  const replies = lastReplies([reply("r1", 500)]);
  assert.equal(workersStrip([restored], replies, [ended(restored.id, 100), ended(restored.id, 950, "updated")]), null);
  assert.equal(workersStrip([restored], replies, [ended(restored.id, 600)])?.stopped, 1);
});

test("with older messages not loaded, a worker that finished before them has its answer out of view", () => {
  const old = task({ state: "done", requestId: "r1", updatedAtMs: 100 });
  const recent = task({ state: "done", requestId: "r2", updatedAtMs: 800 });
  // The newest page starts at 500: r1's answer is older, r2's isn't posted yet.
  const page = lastReplies([reply("r3", 500), reply("r2", 600)]);
  assert.deepEqual(workersStrip([old, recent], page, [], 500)?.rows, [recent.id]);
  // With the whole history loaded, a missing answer is a missing answer.
  assert.deepEqual(workersStrip([old, recent], page, [])?.rows, [old.id, recent.id]);
  // A live worker shows whatever is loaded.
  assert.equal(workersStrip([task({ requestId: "r1", createdAtMs: 1 })], page, [], 500)?.working, 1);
});

test("an answer posted under a message steered into the turn answers the turn's workers too", () => {
  // r2 was steered into r1's running turn: the turn's later replies carry r2, its worker keeps r1.
  const worker = task({ state: "done", requestId: "r1", updatedAtMs: 400 });
  const requests = { r1: { steeredInto: null }, r2: { steeredInto: "r1" }, r3: { steeredInto: null } };
  const answered = lastReplies([reply("r1", 100), reply("r2", 500)], requests);
  assert.equal(workersStrip([worker], answered, []), null);
  // A reply before the worker finished isn't its answer, and an unrelated request's never is.
  assert.equal(workersStrip([worker], lastReplies([reply("r2", 300)], requests), [])?.done, 1);
  assert.equal(workersStrip([worker], lastReplies([reply("r3", 500)], requests), [])?.done, 1);
  // A cycle in stored data ends.
  assert.deepEqual(lastReplies([reply("a", 5)], { a: { steeredInto: "b" }, b: { steeredInto: "a" } }), { a: 5, b: 5 });
});

test("Stop all shows only while a worker runs or waits to run", () => {
  const landing = task({ state: "landing" });
  const ready = task({ state: "readyToLand" });
  assert.equal(workersStrip([landing, ready], {}, [])?.stoppable, false);
  assert.equal(workersStrip([landing, task({ state: "queued" })], {}, [])?.stoppable, true);
});

test("the collapsed strip counts in one line, naming workers once", () => {
  assert.equal(stripWords({ working: 2, waiting: 0, done: 1, stopped: 0 }), "2 workers working · 1 done");
  assert.equal(stripWords({ working: 1, waiting: 1, done: 0, stopped: 0 }), "1 worker working · 1 waiting");
  assert.equal(stripWords({ working: 0, waiting: 0, done: 3, stopped: 0 }), "3 workers done");
  // After Stop all, the stopped ones aren't "done".
  assert.equal(stripWords({ working: 0, waiting: 0, done: 0, stopped: 2 }), "2 workers stopped");
  assert.equal(stripWords({ working: 0, waiting: 0, done: 1, stopped: 1 }), "1 worker done · 1 stopped");
});
