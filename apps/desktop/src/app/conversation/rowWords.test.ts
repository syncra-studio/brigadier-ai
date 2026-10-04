import assert from "node:assert/strict";
import { test } from "node:test";

import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import { namedTasks, workerName } from "@/app/conversation/rowWords";
import type { Task } from "@/ipc/generated";

const tasks = night.tasks as unknown as Record<string, Task>;
const byNumber = (number: number): Task => {
  const task = Object.values(tasks).find((entry) => entry.number === number);
  assert.ok(task);
  return task;
};

test("a check is named for the worker it checks, never by its task-N", () => {
  const title = byNumber(1).title;
  assert.equal(workerName(tasks, byNumber(1)), title);
  assert.equal(workerName(tasks, byNumber(2)), `Review of ${title}`);
  assert.equal(workerName(tasks, byNumber(3)), `Second review of ${title}`);
  assert.equal(workerName(tasks, byNumber(4)), `Check of ${title}`);
});

test("a line's task-N reads as the worker's name, once where the line already quotes it", () => {
  assert.equal(
    namedTasks("Landed task-13 “Make cause 1's fix in the A/B note obey §10.6” on `run`", tasks),
    "Landed “Make cause 1's fix in the A/B note obey §10.6” on `run`",
  );
  assert.equal(
    namedTasks("Sent task-5 back to fix what its checks found", tasks),
    `Sent “${byNumber(5).title}” back to fix what its checks found`,
  );
  assert.equal(
    namedTasks("The checks of task-5's change waited for it", tasks),
    `The checks of “${byNumber(5).title}”'s change waited for it`,
  );
  // A number the session has no worker for stays as it is.
  assert.equal(namedTasks("task-999 is gone", tasks), "task-999 is gone");
});
