import assert from "node:assert/strict";
import { test } from "node:test";

import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import { machineWords, namedTasks, shortWorkerName, workerName } from "@/app/conversation/rowWords";
import type { Task } from "@/ipc/generated";

const tasks = night.tasks as unknown as Record<string, Task>;
const byNumber = (number: number): Task => {
  const task = Object.values(tasks).find((entry) => entry.number === number);
  assert.ok(task);
  return task;
};

test("every worker uses a short plain unique job name, regardless of role", () => {
  const names = Object.values(tasks).map((task) => workerName(tasks, task));
  assert.ok(names.every((name) => name.split(" ").length >= 2 && name.split(" ").length <= 4));
  assert.equal(new Set(names).size, names.length);
  assert.ok(names.every((name) => !/task-\d+|Phase \d/.test(name)));
  assert.equal(shortWorkerName("Build search results with pagination"), "Build search results");
  assert.equal(workerName({}, { ...byNumber(1), title: "File summary", role: "lead" }), "File summary");
});

test("worker names follow the board's tasks: worked out once per record, again for a changed one (issue 401)", () => {
  const first = byNumber(1);
  const name = workerName(tasks, first);
  assert.equal(workerName(tasks, first), name);
  // A new record (as the board makes when a task changes) names the worker by its new title.
  const renamed = { ...tasks, [first.id]: { ...first, title: "Tidy release notes" } };
  assert.equal(workerName(renamed, renamed[first.id]!), "Tidy release notes");
  // A worker the record doesn't hold yet comes after all of them, its name kept unique.
  const extra = { ...first, id: "not-on-board", number: 999, title: first.title };
  assert.notEqual(workerName(tasks, extra), name);
  assert.equal(workerName(tasks, extra), workerName(tasks, extra));
});

test("a line's task-N reads as the worker's name, once where the line already quotes it", () => {
  assert.equal(
    namedTasks("Landed task-13 “Make cause 1's fix in the A/B note obey §10.6” on `run`", tasks),
    "Landed “Make cause 1's fix in the A/B note obey §10.6” on `run`",
  );
  assert.equal(
    namedTasks("Sent task-5 back to fix what its checks found", tasks),
    `Sent “${workerName(tasks, byNumber(5))}” back to fix what its checks found`,
  );
  assert.equal(
    namedTasks("The checks of task-5's change waited for it", tasks),
    `The checks of “${workerName(tasks, byNumber(5))}”'s change waited for it`,
  );
  // A number the session has no worker for stays as it is.
  assert.equal(namedTasks("task-999 is gone", tasks), "task-999 is gone");
  // So does one inside a branch, path, file name or longer word; a sentence's full stop still ends it.
  for (const kept of ["on `brigadier/x/task-13-fix`", "docs/task-13.md", "read task-13.diff", "task-13x", "subtask-13"]) {
    assert.equal(namedTasks(kept, tasks), kept);
  }
  assert.equal(namedTasks("Sent task-5.", tasks), `Sent “${workerName(tasks, byNumber(5))}”.`);
});

test("machine holds name memory separately from heat, while pauses remain heat-only", () => {
  assert.equal(machineWords("waitingToCool", null, "the Mac", "memory"), "Waiting for memory to free up");
  assert.equal(machineWords("waitingToCool", null, "the Mac", "heat"), "Waiting for the Mac to cool down");
  assert.equal(machineWords("waitingToCool", null, "the Mac"), "Waiting for the Mac to cool down");
  assert.equal(machineWords("paused", "cargo test", "the Mac", "heat"), "Paused cargo test to let the Mac cool down");
});
