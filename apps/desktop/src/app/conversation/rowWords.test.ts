import assert from "node:assert/strict";
import { test } from "node:test";

import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import { decisionWords, namedTasks, workerName } from "@/app/conversation/rowWords";
import type { Decision, Task } from "@/ipc/generated";

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
  // Unavailable worker references still never expose an internal id.
  assert.equal(namedTasks("task-999 is gone", tasks), "a worker is gone");
  // So does one inside a branch, path, file name or longer word; a sentence's full stop still ends it.
  for (const kept of ["on `brigadier/x/task-13-fix`", "docs/task-13.md", "read task-13.diff", "task-13x", "subtask-13"]) {
    assert.equal(namedTasks(kept, tasks), kept);
  }
  assert.equal(namedTasks("Sent task-5.", tasks), `Sent “${byNumber(5).title}”.`);
});

test("a decision recorded in older, longer words reads in the board's short ones", () => {
  const decisions = night.decisions as unknown as Decision[];
  const sentBack = decisions.find((decision) => decision.what.startsWith("Sent task-5 back"));
  assert.ok(sentBack);
  assert.deepEqual(decisionWords(sentBack), {
    what: "Sent task-5 back after review (fix 1 of 2)",
    why: "The findings are on its checks.",
  });
  for (const decision of decisions) {
    assert.doesNotMatch(decisionWords(decision).why, /From the (review|verification)|\[not (run|checked)\]/);
  }
  // One that arrived since the board was read is in the short words already.
  const live = { ...sentBack, short: null, what: "Sent task-5 back: 2 review findings (fix 1 of 2)", why: "" };
  assert.deepEqual(decisionWords(live), { what: live.what, why: "" });
});
