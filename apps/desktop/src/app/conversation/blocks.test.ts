import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import { settledSession } from "@/fixtures/settledSession";
import {
  answerIndex,
  type Block,
  type BoardDigest,
  blockSequence,
  buildBlocks,
  endsWithPlan,
  foldsAway,
  foldTurns,
  judgementCall,
  type ThreadNode,
} from "@/app/conversation/blocks";
import { splitReport } from "@/app/conversation/phaseView";
import { checkersOf, checkResult, checksCount, machineWords, taskRowDetail, taskState } from "@/app/conversation/rowWords";
import type { Decision, EventEnvelope, MachineStep, Message, OrchestratorStep, OvernightRun, Plan, Task, UserRequest } from "@/ipc/generated";
import { applyToBoard, emptyBoard } from "@/state/board";

// The board of the first real overnight run (2026-10-03), from its stored events
// (scripts/extract-board-fixture.mjs), in the shape a run has now: one request for the whole run,
// its phases the steps of its plan. Before one row per task, its Phase 1 showed 41 rows and its
// Phase 2 53, most of them "started working" and "finished" lines of checkers.
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

const RUN_REQUEST = "run-09cc7d53-g1";

test("all workers of the run's phases remain visible in its one block, including reviews and verifiers", () => {
  const block = blockOf(RUN_REQUEST);
  const ids = sequence(block).flatMap((entry) => entry.kind === "row" ? entry.row.taskIds ?? [entry.row.taskId] : []);
  const expected = Object.values(tasks).filter((task) => task.requestId === RUN_REQUEST).map((task) => task.id);
  assert.equal(expected.length, 44);
  assert.deepEqual(new Set(ids), new Set(expected));
  assert.equal(block.cards.filter((card) => card.type === "task").length, 0);
});

test("an older task's or plan's checker has its own openable lifecycle row", () => {
  const rows = buildBlocks(messages, {}, false, board, []).flatMap((block) => block.rows);
  assert.deepEqual(new Set(rows.map((row) => row.taskId)), new Set(Object.keys(tasks)));
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
  const plans = blockOf(RUN_REQUEST).cards.filter((card) => card.type === "plan");
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
    (checker) => `task-${checker.number} ${checkResult(checker)}`,
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
  const run = Object.values(night.overnight)[0] as unknown as OvernightRun;
  // Phase 2's records: its tasks, and what the run's request did from when it began.
  const began = board.plans[run.planId ?? ""]?.steps[1]?.startedAtMs ?? 0;
  assert.ok(began > 0);
  const request = "normal-1";
  const ofPhase = (requestId: string | null, atMs: number) => requestId === RUN_REQUEST && atMs >= began;
  const held = byNumber(26).id;
  const normalTasks = Object.fromEntries(
    Object.values(tasks).map((task) => [
      task.id,
      {
        ...task,
        requestId: task.phase === 2 ? request : task.requestId,
        run: null,
        ...(task.id === held ? { state: "readyToLand" as const, landed: null } : {}),
      },
    ]),
  );
  const approved = Object.values(board.plans).find(
    (plan) => ofPhase(plan.requestId, plan.createdAtMs) && plan.state.type === "approved",
  );
  assert.ok(approved);
  const normalPlans: Record<string, Plan> = Object.fromEntries(
    Object.values(board.plans).map((plan) => [
      plan.id,
      { ...plan, requestId: ofPhase(plan.requestId, plan.createdAtMs) ? request : plan.requestId },
    ]),
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
    requests: { [request]: { ...board.requests[RUN_REQUEST]!, id: request, startedAtMs: began, state: { type: "done" } } },
    orchestratorSteps: board.orchestratorSteps
      .filter((step) => ofPhase(step.requestId, step.atMs))
      .map((step) => ({ ...step, requestId: request })),
    decisions: board.decisions
      .filter((decision) => ofPhase(decision.requestId, decision.atMs))
      .map((decision) => ({ ...decision, requestId: request })),
  };
  const block = buildBlocks([user], {}, false, normal, []).find((candidate) => candidate.key === request);
  assert.ok(block);
  return { block, normalTasks, held };
}

test("a normal session includes helper workers and keeps approved plans out of the thread", () => {
  const { block, normalTasks, held } = normalSession();
  const rows = block.rows.flatMap((row) => (row.type === "task" ? [row.taskId] : []));
  // Every worker, including each reviewer and verifier.
  assert.deepEqual(
    rows.map((id) => normalTasks[id]?.number),
    Object.values(normalTasks).filter((task) => task.requestId === block.key).map((task) => task.number),
  );
  // Approved plans at work show in the side panel and the phase pill, not in the thread.
  assert.deepEqual(
    block.cards.filter((card) => card.type === "plan").map((card) => card.id),
    [],
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

test("a plan shows as its document: writing it, then the newest revision in view and the plan as the answer", () => {
  const request = "plan-request";
  const user = { ...messages[0]!, id: request, seq: 320, requestId: request, text: "Add a --version flag" };
  const opening = { ...messages[0]!, id: "opening", seq: 321, role: "assistant" as const, requestId: request, parentId: request, text: "I'll look at the CLI first." };
  const writing: OrchestratorStep = {
    requestId: request,
    position: 400,
    atMs: user.createdAtMs + 5,
    kind: { type: "tool", itemId: "call-1", name: "mcp__brigadier__propose_plan", detail: null, status: "inProgress", throughPosition: 400 },
  };
  const plan = (id: string, position: number, state: Plan["state"]): Plan => ({
    id,
    conversationId: user.conversationId,
    requestId: request,
    position,
    title: "Add a --version flag",
    body: "# Add a --version flag\n\nPrint the version.\n\n## Changes\n- `src/main.rs`",
    steps: [],
    state,
    createdAtMs: user.createdAtMs + position,
    decidedAtMs: null,
  });
  const digest = (plans: Plan[], steps: OrchestratorStep[]): BoardDigest => ({
    ...board,
    tasks: {},
    plans: Object.fromEntries(plans.map((p) => [p.id, p])),
    requests: { [request]: { ...Object.values(board.requests)[0]!, id: request, startedAtMs: user.createdAtMs, state: { type: "done" } } },
    orchestratorSteps: steps,
    decisions: [],
  });
  const cardsOf = (d: BoardDigest) => {
    const block = buildBlocks([user, opening], {}, false, d, []).find((candidate) => candidate.key === request);
    assert.ok(block);
    return { block, cards: block.cards.map((card) => [card.type, card.id, card.keep]) };
  };
  // While the call runs: "Writing plan", and nothing yet the answer.
  assert.deepEqual(cardsOf(digest([], [writing])).cards, [["writingPlan", "writing:400", true]]);
  // The plan arrived before its call reported done: the plan alone.
  assert.deepEqual(cardsOf(digest([plan("p1", 401, { type: "proposed" })], [writing])).cards, [["plan", "p1", true]]);
  // Sent back and proposed again: the newest stays in view, the older folds; the plan is the answer.
  const done = { ...writing, kind: { ...writing.kind, status: "completed" as const } };
  const { block, cards } = cardsOf(
    digest([plan("p1", 401, { type: "rejected", message: "smaller" }), plan("p2", 405, { type: "proposed" })], [done]),
  );
  assert.deepEqual(cards, [["plan", "p1", false], ["plan", "p2", true]]);
  assert.ok(endsWithPlan(sequence(block)));
  // Once a reply comes after it (the work it started), that reply is the answer again.
  const after = { ...opening, id: "after", seq: 500, text: "Done." };
  const later = buildBlocks([user, opening, after], {}, false, digest([plan("p2", 405, { type: "approved", by: "user" })], [done]), []).find((b) => b.key === request);
  assert.ok(later);
  assert.ok(!endsWithPlan(sequence(later)));
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

test("a run's report shows, and copies, as its message was written", () => {
  const run = Object.values(night.overnight)[0] as unknown as OvernightRun;
  const id = run.reportMessageId ?? "";
  const stored = messages.find((message) => message.id === id)?.text ?? "";
  assert.ok(stored.startsWith("**Faster, leaner overnight runs**"));
  const shown = buildBlocks(messages, {}, false, board, [])
    .flatMap((block) => block.texts)
    .find((text) => text.messageId === id);
  assert.equal(shown?.text, stored);
});

test("stored lifecycle events merge when adjacent and of one word: starts with starts, finishes with finishes", () => {
  const user = { ...messages[0]!, id: "life", requestId: "life", seq: 1 };
  const one = { ...byNumber(1), id: "one", requestId: "life", position: 2 };
  const two = { ...byNumber(1), id: "two", requestId: "life", position: 3 };
  const replay: BoardDigest = { ...board, plans: {}, decisions: [], tasks: { one, two },
    orchestratorSteps: [], requests: {}, workerSteps: [
      { taskId: "one", requestId: "life", kind: "started", position: 2, atMs: 10 },
      { taskId: "two", requestId: "life", kind: "started", position: 3, atMs: 20 },
      { taskId: "two", requestId: "life", kind: "finished", position: 4, atMs: 30 },
      { taskId: "one", requestId: "life", kind: "finished", position: 5, atMs: 40 },
      { taskId: "one", requestId: "life", kind: "landed", position: 6, atMs: 50 },
    ] };
  const rows = sequence(buildBlocks([user], {}, false, replay, [])[0]!).flatMap((entry) => entry.kind === "row" ? [entry.row] : []);
  assert.equal(rows.length, 2);
  assert.deepEqual(rows[0]?.taskIds, ["one", "two"]);
  assert.deepEqual(rows.map((row) => row.kind), ["started", "finished"]);
  assert.deepEqual(rows[1]?.taskIds, ["two", "one"]);
});

const teamStep = (position: number, kind: OrchestratorStep["kind"]): OrchestratorStep => ({ requestId: "team", atMs: position, position, kind });

test("the lead managing its team shows: a message, an answer, a stop with its reason, a landing; a stop isn't said twice", () => {
  const user = { ...messages[0]!, id: "team", requestId: "team", seq: 1 };
  const one = { ...byNumber(1), id: "one", requestId: "team", position: 2 };
  const two = { ...byNumber(1), id: "two", requestId: "team", position: 3 };
  const replay: BoardDigest = { ...board, plans: {}, decisions: [], tasks: { one, two }, requests: {},
    orchestratorSteps: [
      teamStep(4, { type: "created", taskId: "one" }),
      teamStep(5, { type: "messaged", taskId: "one", text: "Use the new table." }),
      teamStep(6, { type: "answered", taskId: "two", question: "Which file?", answer: "notes.py", why: "" }),
      teamStep(8, { type: "stopped", taskId: "two", reason: "The other worker covers it." }),
      teamStep(10, { type: "readReport", taskId: "one" }),
      teamStep(11, { type: "landed", taskIds: ["one"], commits: 2, branch: "b", head: "h" }),
    ],
    workerSteps: [
      { taskId: "one", requestId: "team", kind: "started", position: 2, atMs: 10 },
      { taskId: "two", requestId: "team", kind: "started", position: 3, atMs: 20 },
      { taskId: "two", requestId: "team", kind: "stopped", position: 9, atMs: 30 },
    ] };
  const shown = sequence(buildBlocks([user], {}, false, replay, [])[0]!).flatMap((entry) =>
    entry.kind === "row" ? [`row:${entry.row.kind}`] : entry.kind === "orchestrator" ? entry.steps.map((s) => s.kind.type) : []);
  assert.deepEqual(shown, ["row:started", "messaged", "answered", "stopped", "landed"]);
});

const node = (id: string, parentId: string | null, kind: ThreadNode["kind"], state: Block["state"] = "done"): ThreadNode =>
  ({ id, parentId, kind, block: { user: kind === "user" ? {} : null, state } as unknown as Block, head: null });

test("a long thread folds its oldest turns, other versions of a folded turn with it", () => {
  // Turn n is a user node and its reply; turn 2 has a second version (an edit) branching off turn 1.
  const nodes: ThreadNode[] = [];
  for (let turn = 1; turn <= 5; turn += 1) {
    nodes.push(node(`u${turn}`, turn === 1 ? null : `r${turn - 1}`, "user"), node(`r${turn}`, `u${turn}`, "block"));
  }
  nodes.push(node("u2-edit", "r1", "user"), node("r2-edit", "u2-edit", "block"));
  const tree = { nodes, headId: "r5" };
  const folded = foldTurns(tree, 3);
  assert.equal(folded.hidden, 2);
  assert.deepEqual(folded.tree.nodes.map((kept) => kept.id), ["u3", "r3", "u4", "r4", "u5", "r5"]);
  assert.equal(folded.tree.nodes[0]?.parentId, null);
  // Short enough: nothing folds.
  assert.deepEqual(foldTurns(tree, 5), { tree, hidden: 0 });
});

test("a turn still waiting on the user keeps itself and every later turn out of the fold", () => {
  // A reply's block carries its user message too (buildThread): turns are counted by user nodes only.
  const nodes: ThreadNode[] = [];
  for (let turn = 1; turn <= 5; turn += 1) {
    const reply = node(`r${turn}`, `u${turn}`, "block", turn === 2 ? "waiting" : "done");
    nodes.push(node(`u${turn}`, turn === 1 ? null : `r${turn - 1}`, "user"), { ...reply, block: { ...reply.block, user: {} } as Block });
  }
  const tree = { nodes, headId: "r5" };
  const folded = foldTurns(tree, 2);
  assert.equal(folded.hidden, 1);
  assert.deepEqual(folded.tree.nodes.map((kept) => kept.id), ["u2", "r2", "u3", "r3", "u4", "r4", "u5", "r5"]);
  assert.equal(foldTurns({ nodes: nodes.map((kept) => ({ ...kept, block: { ...kept.block, state: "done" } as Block })), headId: "r5" }, 2).hidden, 3);
});

// The user's session of 2026-10-09 (a grill, then the right sidebar and tabs built), from its
// stored events. Its last request ended twice, the same merge question each time: once when the
// work landed, again after a late review's fix landed, while two "Waiting on you" items kept it
// waiting, unfolded.
const grill = JSON.parse(
  readFileSync(new URL("../../fixtures/boards/thread-grill-2026-10-09.events.json", import.meta.url), "utf8"),
) as { conversationId: string; events: EventEnvelope[] };

function grillBlocks(events: readonly EventEnvelope[]) {
  let replayed = emptyBoard(grill.conversationId);
  const said: Message[] = [];
  for (const envelope of events) {
    replayed = applyToBoard(replayed, envelope);
    if (envelope.event.type === "messageAppended") said.push({ ...envelope.event.message, seq: envelope.streamSeq });
  }
  return { board: replayed, blocks: buildBlocks(said, {}, false, replayed, []) };
}

test("a request whose ending was written again shows only the newest, the first in its fold", () => {
  const merge = { branch: "brigadier/9a2b00c9/session", base: "main" };
  const { board: settled, blocks } = grillBlocks(settledSession(grill.conversationId, grill.events, merge));
  // Nothing is listed for the user, and the request is done while the merge card waits.
  assert.deepEqual(settled.waiting, {});
  const last = blocks.at(-1);
  assert.ok(last);
  assert.equal(last.state, "done");
  const card = Object.values(settled.questions).find((question) => question.kind.type === "merge");
  assert.equal(card?.requestId, last.key.replace("request:", ""));
  assert.equal(card?.answeredAtMs, null);
  // Two endings: the newest is the answer, the first folds into the work with the rest.
  const endings = last.texts.filter((text) => text.text.includes("should I merge"));
  assert.equal(endings.length, 2);
  const answer = answerIndex(last.texts, last.state === "done");
  assert.equal(answer, last.texts.length - 1);
  assert.ok(last.texts[answer!]!.text.startsWith("I fixed the four problems the last review found"));
  const shown = sequence(last).filter((entry) => !foldsAway(entry, answer));
  assert.deepEqual(shown.filter((entry) => entry.kind === "text").map((entry) => entry.kind === "text" && entry.index), [answer]);
  const first = last.texts.findIndex((text) => text === endings[0]);
  assert.ok(sequence(last).some((entry) => entry.kind === "text" && entry.index === first && foldsAway(entry, answer)));
  // As recorded, the items kept it waiting: nothing was the answer, and nothing folded.
  const before = grillBlocks(grill.events);
  assert.equal(before.blocks.at(-1)?.state, "waiting");
  assert.equal(Object.keys(before.board.waiting).length, 2);
  assert.equal(answerIndex(before.blocks.at(-1)!.texts, false), null);
});

test("an answer with a Details section shows its head and folds the rest", () => {
  const answer = [
    "The right sidebar and the tabs are on `brigadier/9a2b00c9/session`.",
    "To check: the native save dialog, which the scripted checks can't open.",
    "",
    "### Details",
    "- Desktop tests (190), typecheck, lint and build pass.",
    "- The review found four problems; all four are fixed.",
  ].join("\n");
  assert.deepEqual(splitReport(answer), {
    head: "The right sidebar and the tabs are on `brigadier/9a2b00c9/session`.\nTo check: the native save dialog, which the scripted checks can't open.",
    details: "- Desktop tests (190), typecheck, lint and build pass.\n- The review found four problems; all four are fixed.",
  });
  // Without one, the answer shows whole.
  assert.equal(splitReport("Merged into `main`."), null);
});
