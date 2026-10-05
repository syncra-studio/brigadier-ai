import assert from "node:assert/strict";
import { test } from "node:test";

import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import { type Block, type BoardDigest, blockSequence, buildBlocks, judgementCall } from "@/app/conversation/blocks";
import { reportTexts, shownTexts } from "@/app/conversation/phaseView";
import { checkersOf, checkResult, checksCount, machineWords, taskRowDetail, taskState } from "@/app/conversation/rowWords";
import type { Decision, MachineStep, Message, OrchestratorStep, OvernightRun, Plan, Task, UserRequest } from "@/ipc/generated";

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
  machineSteps: [],
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
    steers: block.steers.map((steer) => ({
      ...steer,
      atMs: steer.message.createdAtMs,
      attachments: steer.message.attachments,
    })),
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

test("Phase 1 of the night: its three attempts and the whole-phase checks; its header says it was verified", () => {
  const shown = lines(blockOf("run-09cc7d53-phase-1-g1"));
  assert.deepEqual(shown, ["task-1", "task-5", "task-13", "text", "checks phase-1"]);
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

test("an older task's row counts every check it had", () => {
  const router = byNumber(26);
  assert.equal(taskState(router).word, "Landed");
  assert.equal(
    taskRowDetail(checkersOf(tasks, [`task:${router.id}`])),
    "checked by 2 reviews + 3 verifies",
  );
  const landing = byNumber(32);
  assert.equal(
    taskRowDetail(checkersOf(tasks, [`task:${landing.id}`])),
    "checked by 2 reviews + 2 verifies",
  );
  const slots = byNumber(37);
  // The run's Stop interrupted its fix round.
  assert.equal(taskState(slots).word, "Stopped");
  // Its checks still failed after the fixes: the change never landed.
  assert.equal(taskState(byNumber(5)).word, "Not landed");
  assert.equal(taskRowDetail(checkersOf(tasks, [`task:${slots.id}`])), "checked by 2 reviews + 1 verify");
  assert.equal(checksCount(checkersOf(tasks, [`task:${slots.id}`])), "2 reviews + 1 verify");
});

test("an older task's checker result reads from its report", () => {
  const router = byNumber(26);
  const results = checkersOf(tasks, [`task:${router.id}`]).map(
    (checker) => `task-${checker.number} ${checkResult(checker, null)}`,
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

/**
 * Phase 2's records replayed as a normal session's request: the same tasks, checks and plans,
 * with no run, one task held for the user, and a second, separate plan.
 */
function normalSession() {
  const phase = "run-09cc7d53-phase-2-g1";
  const request = "normal-1";
  const move = (requestId: string | null) => (requestId === phase ? request : requestId);
  const held = byNumber(26).id;
  const normalTasks = Object.fromEntries(
    Object.values(tasks).map((task) => [
      task.id,
      {
        ...task,
        requestId: move(task.requestId),
        run: null,
        ...(task.id === held ? { state: "readyToLand" as const, landed: null } : {}),
      },
    ]),
  );
  const approved = Object.values(board.plans).find((plan) => plan.requestId === phase && plan.state.type === "approved");
  assert.ok(approved);
  const normalPlans: Record<string, Plan> = Object.fromEntries(
    Object.values(board.plans).map((plan) => [plan.id, { ...plan, requestId: move(plan.requestId) ?? "" }]),
  );
  normalPlans.separate = {
    ...approved,
    id: "separate",
    requestId: request,
    title: "A separate plan",
    position: approved.position + 1,
    steps: [],
  };
  const user = { ...messages[0]!, id: request, seq: 320, requestId: request, text: "Fix the three causes" };
  const normal: BoardDigest = {
    ...board,
    tasks: normalTasks,
    plans: normalPlans,
    requests: { [request]: { ...board.requests[phase]!, id: request, state: { type: "done" } } },
    orchestratorSteps: board.orchestratorSteps
      .filter((step) => step.requestId === phase)
      .map((step) => ({ ...step, requestId: request })),
    decisions: board.decisions
      .filter((decision) => decision.requestId === phase)
      .map((decision) => ({ ...decision, requestId: request })),
  };
  const block = buildBlocks([user], {}, false, normal, []).find((candidate) => candidate.key === request);
  assert.ok(block);
  return { block, normalTasks, held };
}

test("a normal session's request reads the same way: one row per task, its checks and plans folded in", () => {
  const { block, normalTasks, held } = normalSession();
  const rows = block.rows.flatMap((row) => (row.type === "task" ? [row.taskId] : []));
  // Each worker once; none of its checkers.
  assert.deepEqual(
    rows.map((id) => normalTasks[id]?.number),
    [26, 32, 37, 41],
  );
  // The revised plan once, as its newest revision, and the separate plan beside it.
  assert.deepEqual(
    block.cards.filter((card) => card.type === "plan").map((card) => card.id),
    ["01a0ff89-d905-7753-bb7f-b310c324a8f6", "separate"],
  );
  // A task held for the user says so on its row, and its checks still open from it.
  const task = normalTasks[held]!;
  assert.equal(taskState(task).word, "Ready to land");
  assert.equal(taskRowDetail(checkersOf(normalTasks, [`task:${held}`])), "checked by 2 reviews + 3 verifies");
  // Only the orchestrator's own call is a "Decided for you" line in the thread.
  const decided = sequence(block).flatMap((entry) =>
    entry.kind === "orchestrator" ? entry.steps.filter((step) => step.kind.type === "decided") : [],
  );
  assert.equal(decided.length, 1);
});

test("machine rows show in their request's block, each on its own line", () => {
  const request = "machine-request";
  const user = { ...messages[0]!, id: request, seq: 320, requestId: request, text: "Build it" };
  const step = (kind: MachineStep["kind"], at: number, command: string | null): MachineStep => ({
    kind,
    requestId: request,
    taskId: null,
    command,
    atMs: user.createdAtMs + at,
    position: 400 + at,
  });
  const digest: BoardDigest = {
    ...board,
    tasks: {},
    plans: {},
    requests: { [request]: { ...Object.values(board.requests)[0]!, id: request, startedAtMs: user.createdAtMs, state: { type: "done" } } },
    orchestratorSteps: [],
    decisions: [],
    machineSteps: [step("waitingToCool", 1, null), step("paused", 2, "cargo test"), step("resumed", 3, "cargo test")],
  };
  const block = buildBlocks([user], {}, false, digest, []).find((candidate) => candidate.key === request);
  assert.ok(block);
  const rows = sequence(block).flatMap((entry) =>
    entry.kind === "orchestrator"
      ? [entry.steps.map((s) => (s.kind.type === "machine" ? machineWords(s.kind.machine, s.kind.command, "the Mac") : ""))]
      : [],
  );
  assert.deepEqual(rows, [
    ["Waiting for the Mac to cool down"],
    ["Paused cargo test to let the Mac cool down"],
    ["Resumed cargo test"],
  ]);
});

test("a run's decision shows in the thread unless its kind says it is a phase's outcome", () => {
  const verified = board.decisions.find((decision) => decision.kind === "phaseOutcome");
  assert.ok(verified, "the night's “Verified phase 1” decision");
  assert.equal(judgementCall(verified), false);
  // The kind decides, not the words: a run's call that happens to read like an outcome still shows.
  assert.equal(judgementCall({ ...verified, kind: "routine" }), true);
  const routine = board.decisions.find((decision) => decision.source.type === "task");
  assert.ok(routine);
  assert.equal(judgementCall(routine), false);
});

test("a report rendered again shows, and copies, in place of the text it was written with", () => {
  const run = Object.values(night.overnight)[0] as unknown as OvernightRun;
  const id = run.reportMessageId ?? "";
  const stored = messages.find((message) => message.id === id)?.text ?? "";
  assert.ok(stored.includes("### Phase 1 · Measure — ✓ verified"), "the stored report is the old one");
  // Nothing rendered again: the message's own text.
  const fullText = {};
  assert.equal(shownTexts(fullText, reportTexts({ [run.id]: { ...run, reportText: null } })), fullText);
  // The fixture's run holds its report as the daemon renders it again.
  const again = run.reportText ?? "";
  assert.ok(again.startsWith("**Faster, leaner overnight runs**: stopped by you at 07:04. 1 of 3 phases verified."));
  const texts = shownTexts({}, reportTexts({ [run.id]: run }));
  const shown = buildBlocks(messages, texts, false, board, [])
    .flatMap((block) => block.texts)
    .find((text) => text.messageId === id);
  assert.equal(shown?.text, again);
  // The stored message is untouched.
  assert.equal(messages.find((message) => message.id === id)?.text, stored);
});
