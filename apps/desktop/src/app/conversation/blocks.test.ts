import assert from "node:assert/strict";
import { test } from "node:test";

import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import { type Block, type BoardDigest, blockSequence, buildBlocks } from "@/app/conversation/blocks";
import { checkersOf, checkResult, checksCount, taskRowDetail, taskState } from "@/app/conversation/rowWords";
import type { Decision, Message, OrchestratorStep, Plan, Task, UserRequest } from "@/ipc/generated";

// The board of the first real overnight run (2026-10-03), from its stored events
// (scripts/extract-board-fixture.mjs). Before one row per task, its Phase 1 showed 41 rows and
// its Phase 2 53, most of them "started working" and "finished" lines of checkers.
const tasks = night.tasks as unknown as Record<string, Task>;
const board: BoardDigest = {
  tasks,
  approvals: {},
  questions: {},
  plans: night.plans as unknown as Record<string, Plan>,
  requests: night.requests as unknown as Record<string, UserRequest>,
  orchestratorSteps: night.orchestratorSteps as unknown as OrchestratorStep[],
  decisions: night.decisions as unknown as Decision[],
  compactions: {},
  runRequest: null,
  streaming: null,
};
const messages = night.messages as unknown as Message[];
const byNumber = (number: number): Task => {
  const task = Object.values(tasks).find((candidate) => candidate.number === number);
  assert.ok(task, `task-${number}`);
  return task;
};

function blockOf(key: string): Block {
  const block = buildBlocks(messages, {}, false, board, []).find((candidate) => candidate.key === key);
  assert.ok(block, key);
  return block;
}

/** The block as its message metadata carries it. */
function sequence(block: Block) {
  return blockSequence({
    ...block,
    steers: block.steers.map((steer) => ({ ...steer, atMs: steer.message.createdAtMs })),
  });
}

/** Each entry as a short line: what the thread shows, top to bottom. */
function lines(block: Block): string[] {
  return sequence(block).map((entry) => {
    switch (entry.kind) {
      case "row":
        return entry.row.type === "task" ? `task-${tasks[entry.row.taskId]?.number}` : `checks ${entry.row.phaseId}`;
      case "orchestrator":
        return entry.steps.map((step) => step.kind.type).join("+");
      case "card":
        return `${entry.card.type} card`;
      default:
        return entry.kind;
    }
  });
}

test("Phase 2 of the night shows one row per worker task: 7 rows instead of 53", () => {
  const shown = lines(blockOf("run-09cc7d53-phase-2-g1"));
  assert.deepEqual(shown, [
    "plan card",
    "task-26",
    "task-32",
    "task-37",
    "task-41",
    "decided",
    "text",
  ]);
});

test("Phase 1 of the night: its three attempts, the whole-phase checks and its one judgement call", () => {
  const shown = lines(blockOf("run-09cc7d53-phase-1-g1"));
  assert.deepEqual(shown, ["task-1", "task-5", "task-13", "text", "checks phase-1", "decided"]);
});

test("no checker has a row of its own", () => {
  const rows = buildBlocks(messages, {}, false, board, []).flatMap((block) => block.rows);
  const shown = new Set(rows.flatMap((row) => (row.type === "task" ? [row.taskId] : [])));
  for (const task of Object.values(tasks)) {
    assert.equal(shown.has(task.id), task.gateLink === null, `task-${task.number}`);
  }
  assert.equal(shown.size, 7);
});

test("a decision never folds into a summary of reads", () => {
  for (const block of buildBlocks(messages, {}, false, board, [])) {
    for (const entry of sequence(block)) {
      if (entry.kind !== "orchestrator" || entry.steps.length < 2) continue;
      assert.ok(entry.steps.every((step) => step.kind.type !== "decided"));
    }
  }
});

test("a revised plan shows once, as its newest revision", () => {
  const plans = blockOf("run-09cc7d53-phase-2-g1").cards.filter((card) => card.type === "plan");
  assert.deepEqual(
    plans.map((card) => card.id),
    ["01a0ff89-d905-7753-bb7f-b310c324a8f6"],
  );
});

test("a task's row counts every check it had and its fixes", () => {
  const router = byNumber(26);
  assert.equal(taskState(router).word, "Landed");
  assert.equal(
    taskRowDetail(router, checkersOf(tasks, [`task:${router.id}`])),
    "checked by 2 reviews + 3 verifies",
  );
  const landing = byNumber(32);
  assert.equal(
    taskRowDetail(landing, checkersOf(tasks, [`task:${landing.id}`])),
    "checked by 2 reviews + 2 verifies · 1 fix",
  );
  const slots = byNumber(37);
  // The run's Stop interrupted its fix round.
  assert.equal(taskState(slots).word, "Stopped");
  // Its checks still failed after the fixes: the change never landed.
  assert.equal(taskState(byNumber(5)).word, "Not landed");
  assert.equal(taskRowDetail(slots, checkersOf(tasks, [`task:${slots.id}`])), "checked by 2 reviews + 1 verify · 1 fix");
  assert.equal(checksCount(checkersOf(tasks, [`task:${slots.id}`])), "2 reviews + 1 verify");
});

test("a checker's result reads from its round, or from its report once the round moved on", () => {
  const router = byNumber(26);
  const results = checkersOf(tasks, [`task:${router.id}`]).map(
    (checker) => `task-${checker.number} ${checkResult(checker, router.gate)}`,
  );
  assert.deepEqual(results, [
    "task-27 passed",
    "task-28 couldn’t run its checks",
    "task-29 couldn’t run its checks",
    "task-30 passed",
    "task-31 passed",
  ]);
});

test("the whole-phase checks of phase 1 open from one row, with the verifier, reviewer and judge", () => {
  const owner = "phase:01a0fefa-e29d-74c7-ac52-3d3409cc7d53:phase-1";
  assert.equal(checksCount(checkersOf(tasks, [owner])), "1 review + 1 verify + 1 judge");
});
