import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import { settledSession } from "@/fixtures/settledSession";
import {
  anchorMessage,
  answerIndex,
  answeringIndex,
  type Block,
  blockFinished,
  type BoardDigest,
  blockSequence,
  foldsAway,
  foldTurns,
  turnsFrom,
  heldForWait,
  isUpdate,
  judgementCall,
  peerOf,
  reopenedSince,
  splitReport,
  type SequenceEntry,
  type ThreadNode,
  updateIndices,
  withoutUpdateMark,
} from "@/app/conversation/blocks";
import { cliBlocks } from "@/app/conversation/cliBlocks";
import { checkersOf, checkResult, checksCount, machineWords, taskRowDetail, taskState } from "@/app/conversation/rowWords";
import { fixtureView } from "@/fixtures/cliView";
import type { Decision, EventEnvelope, MachineStep, Message, OrchestratorStep, PeerMessage, StreamingMessage, Task, UserRequest, WorkerStep } from "@/ipc/generated";
import { applyToBoard, type Board, emptyBoard } from "@/state/board";

/**
 * The thread's blocks as the CLI's session file would draw them: the stored messages and the
 * board's tool steps as its `ThreadView`, with the requests and the streaming reply that shape
 * its turns, and the board's own items placed in them.
 */
function blocksOf(
  messages: readonly Message[],
  digest: BoardDigest,
  more: { requests?: Readonly<Record<string, UserRequest>>; streaming?: StreamingMessage | null } = {},
): Block[] {
  const view = fixtureView(messages, { ...digest, requests: more.requests ?? {}, streaming: more.streaming ?? null, thinking: [] } as unknown as Board);
  return cliBlocks(view, "c", digest);
}

// The board of the first real overnight run (2026-10-03), from its stored events
// (scripts/extract-board-fixture.mjs): one request for the whole run. Before one row per task,
// its Phase 1 showed 41 rows and its Phase 2 53, most of them "started working" and "finished"
// lines of checkers.
const tasks = night.tasks as unknown as Record<string, Task>;
const board: BoardDigest = {
  tasks,
  questions: {},
  orchestratorSteps: night.orchestratorSteps as unknown as OrchestratorStep[],
  machineSteps: [],
  decisions: night.decisions as unknown as Decision[],
  compactions: {},
};
const requests = night.requests as unknown as Record<string, UserRequest>;
const messages = night.messages as unknown as Message[];
const byNumber = (number: number): Task => {
  const task = Object.values(tasks).find((candidate) => candidate.number === number);
  assert.ok(task, `task-${number}`);
  return task;
};

/** The night's one turn: the user's /overnight message, which the run's work went on from. */
function runBlock(): Block {
  const blocks = blocksOf(messages, board, { requests });
  assert.equal(blocks.length, 1);
  return blocks[0]!;
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

/** A request record for a test's own user message, as its turn ended. */
const requestOf = (id: string, startedAtMs: number, state: UserRequest["state"] = { type: "done" }): Record<string, UserRequest> =>
  ({ [id]: { ...requests[RUN_REQUEST]!, id, startedAtMs, endedAtMs: startedAtMs, state } });

test("all workers of the run's phases remain visible in its one block, including reviews and verifiers", () => {
  const block = runBlock();
  const ids = sequence(block).flatMap((entry) => entry.kind === "row" ? entry.row.taskIds ?? [entry.row.taskId] : []);
  const expected = Object.values(tasks).filter((task) => task.requestId === RUN_REQUEST).map((task) => task.id);
  assert.equal(expected.length, 44);
  assert.deepEqual(new Set(ids), new Set(expected));
  assert.equal(block.cards.filter((card) => card.type === "task").length, 0);
});

test("an older task's or plan's checker has its own openable lifecycle row", () => {
  const rows = blocksOf(messages, board, { requests }).flatMap((block) => block.rows);
  assert.deepEqual(new Set(rows.map((row) => row.taskId)), new Set(Object.keys(tasks)));
});

test("a decision never folds into a summary of reads", () => {
  for (const block of blocksOf(messages, board, { requests })) {
    for (const entry of sequence(block)) {
      if (entry.kind !== "orchestrator" || entry.steps.length < 2) continue;
      assert.ok(entry.steps.every((step) => step.kind.type !== "decided"));
    }
  }
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
 * Phase 2's records replayed as a normal session's request: the same tasks and checks, with no
 * run and one task held for the user.
 */
function normalSession() {
  // Phase 2's records: its tasks, and what the run's request did from when it began, as stored.
  const run = Object.values(night.overnight)[0] as unknown as { planId: string | null };
  const plans = night.plans as unknown as Record<string, { steps: { startedAtMs: number | null }[] }>;
  const began = plans[run.planId ?? ""]?.steps[1]?.startedAtMs ?? 0;
  assert.ok(began > 0);
  const request = "normal-1";
  const ofPhase = (requestId: string | null, atMs: number) => requestId === RUN_REQUEST && atMs >= began;
  const held = byNumber(26).id;
  const normalTasks = Object.fromEntries(
    Object.values(tasks).map((task) => [
      task.id,
      {
        ...task,
        requestId: (task as Task & { phase?: number | null }).phase === 2 ? request : task.requestId,
        ...(task.id === held ? { state: "readyToLand" as const, landed: null } : {}),
      },
    ]),
  );
  const user = { ...messages[0]!, id: request, seq: 320, requestId: request, text: "Fix the three causes" };
  // Only Phase 2's records: the CLI's one turn here would take Phase 1's (no turn names its run) by time.
  const ofRequest = <T extends { requestId: string | null }>(records: Record<string, T>) =>
    Object.fromEntries(Object.entries(records).filter(([, record]) => record.requestId === request));
  const normal: BoardDigest = {
    ...board,
    tasks: ofRequest(normalTasks),
    orchestratorSteps: board.orchestratorSteps
      .filter((step) => ofPhase(step.requestId, step.atMs))
      .map((step) => ({ ...step, requestId: request })),
    decisions: board.decisions
      .filter((decision) => ofPhase(decision.requestId, decision.atMs))
      .map((decision) => ({ ...decision, requestId: request })),
  };
  const block = blocksOf([user], normal, { requests: requestOf(request, began) }).find((candidate) => candidate.key === request);
  assert.ok(block);
  return { block, normalTasks, held };
}

test("a normal session includes helper workers", () => {
  const { block, normalTasks, held } = normalSession();
  const rows = block.rows.flatMap((row) => (row.type === "task" ? [row.taskId] : []));
  // Every worker, including each reviewer and verifier.
  assert.deepEqual(
    rows.map((id) => normalTasks[id]?.number),
    Object.values(normalTasks).filter((task) => task.requestId === block.key).map((task) => task.number),
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
  const step = (kind: MachineStep["kind"], at: number, command: string | null, reason: MachineStep["reason"] = "heat"): MachineStep => ({
    kind,
    reason,
    requestId: request,
    taskId: null,
    command,
    atMs: user.createdAtMs + at,
    position: 400 + at,
  });
  const digest: BoardDigest = {
    ...board,
    tasks: {},
    orchestratorSteps: [],
    decisions: [],
    machineSteps: [step("waitingToCool", 1, null), step("paused", 2, "cargo test"), step("resumed", 3, "cargo test"), step("waitingToCool", 4, "cargo test", "memory")],
  };
  const block = blocksOf([user], digest, { requests: requestOf(request, user.createdAtMs) }).find((candidate) => candidate.key === request);
  assert.ok(block);
  const rows = sequence(block).flatMap((entry) =>
    entry.kind === "orchestrator"
      ? [entry.steps.map((s) => (s.kind.type === "machine" ? machineWords(s.kind.machine, s.kind.command, "the Mac", s.kind.reason) : ""))]
      : [],
  );
  assert.deepEqual(rows, [
    ["Waiting for the Mac to cool down"],
    ["Paused cargo test to let the Mac cool down"],
    ["Resumed cargo test"],
    ["Waiting for memory to free up"],
  ]);
});

test("a late event's update line shows under the answer, never as a new answer", () => {
  const request = "late-request";
  const user = { ...messages[0]!, id: request, seq: 600, requestId: request, text: "Add the Configuration page" };
  const reply = (id: string, seq: number, text: string) =>
    ({ ...messages[0]!, id, seq, role: "assistant" as const, requestId: request, parentId: request, text });
  const opening = reply("opening", 601, "Starting two workers.");
  const ending = reply("ending", 610, "The Configuration page is in. Merge it?");
  const update = reply("update", 620, "Update: the review's two fixes landed; nothing open.");
  const bold = reply("bold", 630, "**Update:** the second review is clean.");
  const digest: BoardDigest = {
    ...board,
    tasks: {},
    orchestratorSteps: [],
    decisions: [],
  };
  const done = { requests: requestOf(request, user.createdAtMs) };
  const block = blocksOf([user, opening, ending, update, bold], digest, done).find((candidate) => candidate.key === request);
  assert.ok(block);
  assert.deepEqual(block.texts.map((text) => text.update), [false, false, true, true]);
  // The ending stays the answer; both update lines show under it, in order, and don't fold.
  const answer = answerIndex(block.texts, true);
  assert.equal(block.texts[answer!]!.messageId, "ending");
  assert.deepEqual(updateIndices(block.texts, answer), [2, 3]);
  const shown = sequence(block).filter((entry) => entry.kind === "text" && !foldsAway(entry, answer));
  assert.deepEqual(shown.map((entry) => entry.kind === "text" && entry.index), [1, 2, 3]);
  // The opening line still folds into the work.
  assert.ok(sequence(block).some((entry) => entry.kind === "text" && entry.index === 0 && foldsAway(entry, answer)));
  // An ending written again is still the answer, the first one folded.
  const again = reply("again", 640, "The Configuration page is in, with the review's fixes. Merge it?");
  const rewritten = blocksOf([user, opening, ending, update, again], digest, done).find((candidate) => candidate.key === request)!;
  assert.equal(rewritten.texts[answerIndex(rewritten.texts, true)!]!.messageId, "again");
  assert.deepEqual(updateIndices(rewritten.texts, answerIndex(rewritten.texts, true)), []);
  // Words that only mention an update aren't one; a block of update lines alone answers with its last.
  assert.equal(isUpdate("The update: done"), false);
  assert.equal(isUpdate("  update: lower case"), true);
  assert.equal(answerIndex([{ update: true }, { update: true }], true), 1);
  assert.equal(answerIndex([{ update: true }], false), null);
});

test("an ending that opens with Update: is the answer, not an update line under the opening line", () => {
  const request = "update-ending";
  const user = { ...messages[0]!, id: request, seq: 700, requestId: request, text: "Fix the rounding" };
  const reply = (id: string, seq: number, text: string) =>
    ({ ...messages[0]!, id, seq, role: "assistant" as const, requestId: request, parentId: request, text });
  const opening = reply("opening", 701, "Starting on the rounding fix.");
  const ending = reply("ending", 710, "Update: the session's full checks still don't pass.\n\nThe fix itself is in.\n\n### Details\n- Unit tests pass.");
  const digest: BoardDigest = {
    ...board,
    tasks: {},
    orchestratorSteps: [],
    decisions: [],
  };
  const done = { requests: requestOf(request, user.createdAtMs) };
  const block = blocksOf([user, opening, ending], digest, done).find((candidate) => candidate.key === request)!;
  assert.deepEqual(block.texts.map((text) => text.update), [false, false]);
  const answer = answerIndex(block.texts, true);
  assert.equal(block.texts[answer!]!.messageId, "ending");
  assert.deepEqual(updateIndices(block.texts, answer), []);
  // A short update line is still one, a list under it too; paragraphs or a heading make an ending.
  assert.equal(isUpdate("Update: CI passed on c9cb8b3.\n- build\n- tests"), true);
  assert.equal(isUpdate("Update: done.\n\nMore words."), false);
  assert.equal(isUpdate("Update: done.\n### Details\n- tests"), false);
});

test("a late note keeps its marker for the app and reads as plain words, the answer staying the answer", () => {
  // An answer, then a late note: the answer stays in view, the note under it.
  const texts = [{ update: false }, { update: isUpdate("Update: CI passed on c9cb8b3.") }];
  assert.equal(answerIndex(texts, true), 0);
  assert.deepEqual(updateIndices(texts, 0), [1]);
  assert.equal(withoutUpdateMark("Update: CI passed on c9cb8b3."), "CI passed on c9cb8b3.");
  assert.equal(withoutUpdateMark("**Update:** the review's two fixes landed."), "The review's two fixes landed.");
});

test("a working request shows a reply written after its work in place: only a done one has an answer", () => {
  const request = "owed-request";
  const user = { ...messages[0]!, id: request, seq: 600, requestId: request, text: "Add a note, and merge it when done." };
  const reply = (id: string, seq: number, text: string) =>
    ({ ...messages[0]!, id, seq, role: "assistant" as const, requestId: request, parentId: request, text });
  const landed: Task = { ...byNumber(1), id: "one", requestId: request, position: 601, state: "landed" };
  const digest: BoardDigest = {
    ...board,
    decisions: [],
    tasks: { one: landed },
    orchestratorSteps: [
      { requestId: request, atMs: 603, position: 603, kind: { type: "landed", taskIds: ["one"], commits: 1, branch: "b", head: "h" } },
    ],
  };
  const turn = (state: UserRequest["state"], streaming: string | null) => ({
    requests: requestOf(request, user.createdAtMs, state),
    streaming: streaming === null ? null : { messageId: "live", text: streaming, requestId: request, startedAtMs: 0 },
  });
  const requestBlock = (said: Message[], more: ReturnType<typeof turn>) => {
    const found = blocksOf([user, ...said], digest, more).find((candidate) => candidate.key === request);
    assert.ok(found);
    return found;
  };
  // The thread writes a line after the landing, its worker over, while the merge it holds waits for a review.
  const streaming = requestBlock([], turn({ type: "working" }, "Diff looks right."));
  assert.deepEqual(streaming.texts.map((text) => text.messageId), ["live"]);
  assert.equal(blockFinished(streaming.state), false);
  assert.equal(answerIndex(streaming.texts, blockFinished(streaming.state)), null);
  // Its turn ended and the request works on (a held merge): the line is no answer.
  const said = reply("said", 604, "Diff looks right.");
  const between = requestBlock([said], turn({ type: "working" }, null));
  assert.equal(answerIndex(between.texts, blockFinished(between.state)), null);
  // Done at its real ending: the last reply is the answer, and the line folds into the work.
  const done = requestBlock([said, reply("ending", 620, "Merged the note into main.")], turn({ type: "done" }, null));
  const answer = answerIndex(done.texts, blockFinished(done.state));
  assert.equal(done.texts[answer!]!.text, "Merged the note into main.");
  assert.ok(sequence(done).some((entry) => entry.kind === "text" && entry.index === 0 && foldsAway(entry, answer)));
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
  const run = Object.values(night.overnight)[0] as unknown as { reportMessageId: string | null };
  const id = run.reportMessageId ?? "";
  const stored = messages.find((message) => message.id === id)?.text ?? "";
  assert.ok(stored.startsWith("**Faster, leaner overnight runs**"));
  const shown = blocksOf(messages, board, { requests })
    .flatMap((block) => block.texts)
    .find((text) => text.messageId === id);
  assert.equal(shown?.text, stored);
});

test("workers started together are one row, which says they finished once they all have", () => {
  const user = { ...messages[0]!, id: "life", requestId: "life", seq: 1 };
  const one = { ...byNumber(1), id: "one", requestId: "life", position: 2 };
  const two = { ...byNumber(1), id: "two", requestId: "life", position: 3 };
  const steps: WorkerStep[] = [
    { taskId: "one", requestId: "life", kind: "started", position: 2, atMs: 10 },
    { taskId: "two", requestId: "life", kind: "started", position: 3, atMs: 20 },
    { taskId: "two", requestId: "life", kind: "finished", position: 4, atMs: 30 },
    { taskId: "one", requestId: "life", kind: "finished", position: 5, atMs: 40 },
    { taskId: "one", requestId: "life", kind: "landed", position: 6, atMs: 50 },
  ];
  const rows = (workerSteps: WorkerStep[]) => {
    const replay: BoardDigest = { ...board, decisions: [], tasks: { one, two }, orchestratorSteps: [], workerSteps };
    return sequence(blocksOf([user], replay)[0]!).flatMap((entry) => entry.kind === "row" ? [entry.row] : []);
  };
  const all = rows(steps);
  assert.equal(all.length, 1);
  assert.deepEqual([all[0]?.taskIds, all[0]?.kind, all[0]?.wave], [["one", "two"], "finished", true]);
  // One of them finished so far: the start's row stands, and the finish has its own.
  const half = rows(steps.slice(0, 3));
  assert.deepEqual(half.map((row) => [row.kind, row.taskIds ?? [row.taskId]]), [["started", ["one", "two"]], ["finished", ["two"]]]);
});

const teamStep = (position: number, kind: OrchestratorStep["kind"]): OrchestratorStep => ({ requestId: "team", atMs: position, position, kind });

test("the lead managing its team shows: a message, an answer, a stop with its reason, a landing; a stop isn't said twice", () => {
  const user = { ...messages[0]!, id: "team", requestId: "team", seq: 1 };
  const one = { ...byNumber(1), id: "one", requestId: "team", position: 2 };
  const two = { ...byNumber(1), id: "two", requestId: "team", position: 3 };
  const replay: BoardDigest = { ...board, decisions: [], tasks: { one, two },
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
  const shown = sequence(blocksOf([user], replay)[0]!).flatMap((entry) =>
    entry.kind === "row" ? [`row:${entry.row.kind}`] : entry.kind === "orchestrator" ? entry.steps.map((s) => s.kind.type) : []);
  assert.deepEqual(shown, ["row:started", "messaged", "answered", "stopped", "landed"]);
});

test("old records of a Full access suggestion, a suggested setting, a session offer and a CI watch show nothing", () => {
  const user = { ...messages[0]!, id: "team", requestId: "team", seq: 1 };
  const replay: BoardDigest = { ...board, decisions: [], tasks: {}, workerSteps: [],
    orchestratorSteps: [
      teamStep(2, { type: "fullAccessSuggested", reason: "Installing Homebrew writes outside the project." }),
      teamStep(3, { type: "settingSuggested", change: { setting: "browser", on: true }, reason: "Checking the page needs the built-in browser." }),
      teamStep(4, { type: "sessionOffered", id: "o1", title: "Fix uploads", task: "Find why uploads stall at 99%." }),
      teamStep(5, { type: "ciWatch", repo: "acme/app", branch: "main", sha: "97c298d", waiting: false, runs: [] }),
    ] };
  const shown = sequence(blocksOf([user], replay)[0]!).flatMap((entry) =>
    entry.kind === "orchestrator" ? entry.steps.map((s) => s.kind) : []);
  assert.deepEqual(shown, []);
});

test("the lead's feedback offer stays in view as its card; the user's answer shows no row", () => {
  const user = { ...messages[0]!, id: "team", requestId: "team", seq: 1 };
  const offer = { type: "feedbackOffered", id: "f1", kind: "bug", title: "Stop leaves the tab spinning", body: "After Stop the tab spins." } as const;
  const replay: BoardDigest = { ...board, decisions: [], tasks: {}, workerSteps: [],
    orchestratorSteps: [
      teamStep(3, offer),
      teamStep(4, { type: "feedbackAnswered", id: "f1", sent: false }),
    ] };
  const entries = sequence(blocksOf([user], replay)[0]!);
  const shown = entries.flatMap((entry) => entry.kind === "orchestrator" ? entry.steps.map((s) => s.kind) : []);
  assert.deepEqual(shown, [offer]);
  const kept = entries.filter((entry) => entry.kind !== "text" && !foldsAway(entry, 0));
  assert.deepEqual(kept.flatMap((entry) => entry.kind === "orchestrator" ? entry.steps.map((step) => step.kind) : []), [offer]);
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

test("the turns a thread must hold to show a row count from that row's turn to the end", () => {
  const nodes: ThreadNode[] = [];
  for (let turn = 1; turn <= 5; turn += 1) {
    nodes.push(node(`u${turn}`, turn === 1 ? null : `r${turn - 1}`, "user"), node(`r${turn}`, `u${turn}`, "block"));
  }
  const tree = { nodes, headId: "r5" };
  assert.equal(turnsFrom(tree, "u5"), 1);
  assert.equal(turnsFrom(tree, "r4"), 2);
  assert.equal(turnsFrom(tree, "u2"), 4);
  assert.equal(turnsFrom(tree, "missing"), null);
  // Held from there, the row is in the list.
  assert.ok(foldTurns(tree, turnsFrom(tree, "r2")!).tree.nodes.some((kept) => kept.id === "r2"));
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
// work landed, again after a late review's fix landed, while it was kept waiting, unfolded.
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
  return { board: replayed, blocks: cliBlocks(fixtureView(said, replayed), grill.conversationId, replayed) };
}

test("a request whose ending was written again shows only the newest, the first in its fold", () => {
  const merge = { branch: "brigadier/9a2b00c9/session", base: "main" };
  const { board: settled, blocks } = grillBlocks(settledSession(grill.conversationId, grill.events, merge));
  // The request is done while the merge card waits.
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
  // As recorded, it kept waiting: nothing was the answer, and nothing folded.
  const before = grillBlocks(grill.events);
  assert.equal(blockFinished(before.blocks.at(-1)!.state), false);
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
  // Only the heading itself folds: a line that merely mentions it doesn't, nor an empty fold.
  assert.equal(splitReport("See ### Details below.\n"), null);
  assert.equal(splitReport("Done.\n\n### Details\n"), null);
  assert.equal(splitReport("Done.\n\n### Details"), null);
  // A model's own heading folds too, at another level or with a colon (#198).
  for (const heading of ["## Details", "#### Details", "### Details:"]) {
    assert.deepEqual(splitReport(`Done.\n\n${heading}\n- tests pass\n`), { head: "Done.", details: "- tests pass" }, heading);
  }
  assert.equal(splitReport("Done.\n\n# Details\n- tests\n"), null);
});

const branchMessage = (id: string, parentId: string | null, role: Message["role"]): Message =>
  ({ id, conversationId: "c", seq: 0, role, text: id, blob: null, createdAtMs: 0, attachments: [], mentions: [], model: null, requestId: null, parentId });

// ---- Messages between sessions (SESSION-MESSAGING-PLAN.md §6) ------------------------------------

/** A message at `seq` of a session's stream, `seq` seconds in. */
const said = (id: string, seq: number, role: Message["role"], requestId: string, more: Partial<Message> = {}): Message =>
  ({ ...branchMessage(id, null, role), seq, createdAtMs: seq * 1000, requestId, ...more });
/** Another session's message as this one received it. */
const fromPeer = (id: string, seq: number, requestId: string, replyTo?: string): Message =>
  said(id, seq, "peer", requestId, { peer: { messageId: id, from: "api", ...(replyTo && { replyTo }) } });
const peerRequest = (id: string, seq: number, more: Partial<UserRequest> = {}): UserRequest => ({
  id, conversationId: "c", preview: id, state: { type: "done" }, startedAtMs: seq * 1000, endedAtMs: null,
  steeredInto: null, steeredAfter: null, undo: null, worked: [{ fromMs: seq * 1000, toMs: null }], quotaWait: false, ...more,
} as UserRequest);
const record = (messageId: string, position: number, requestId: string, state: PeerMessage["state"], outgoing = true): PeerMessage => ({
  messageId, outgoing, peer: "docs", requestId, state, text: messageId, blob: null,
  sentAtMs: position * 1000, settled: false, position, automatic: false,
});
const peerDigest = (peerMessages: PeerMessage[] = [], orchestratorSteps: OrchestratorStep[] = []): BoardDigest => ({
  tasks: {}, questions: {}, decisions: [], machineSteps: [], compactions: {}, orchestratorSteps,
  peerMessages: Object.fromEntries(peerMessages.map((peer) => [peer.messageId, peer])),
});
const requestsOf = (...records: UserRequest[]) => ({ requests: Object.fromEntries(records.map((request) => [request.id, request])) });
const keyed = (blocks: Block[], key: string): Block => {
  const block = blocks.find((candidate) => candidate.key === key);
  assert.ok(block, key);
  return block;
};
/** An overnight run's turn: the user's message it went on from, then the run's replies. */
const runThread = [said("u1", 1, "user", "u1"), said("run-1-said", 2, "assistant", "run-1"), said("run-1-done", 5, "assistant", "run-1")];
const runRequests = requestsOf(peerRequest("u1", 1), peerRequest("run-1", 1));

test("a session's sent messages are rows of the turn that sent them, in every state, with a refused send's step", () => {
  const states: PeerMessage["state"][] = [
    { type: "waitingForOk" }, { type: "pending" }, { type: "delivered" }, { type: "seen" }, { type: "replied" },
    { type: "declined" }, { type: "dropped", reason: "deleted" },
  ];
  const sent = states.map((state, index) => record(`m${index}`, 2 + index * 2, "a1", state));
  // Another session's received record is no row: its message shows in the thread.
  const incoming = record("in", 30, "a1", { type: "seen" }, false);
  const refusal: OrchestratorStep = { requestId: "a1", atMs: 20_000, position: 20, kind: { type: "messageRefused", title: "Billing", reason: "Billing is archived." } };
  const blocks = blocksOf([said("a1", 1, "user", "a1")], peerDigest([...sent, incoming], [refusal]), requestsOf(peerRequest("a1", 1)));
  const steps = keyed(blocks, "a1").orchestratorSteps.map((step) => step.kind.type === "peerSent" ? step.kind.messageIds.join(" ") : step.kind.type);
  assert.deepEqual(steps, ["m0", "m1", "m2", "m3", "m4", "m5", "m6", "messageRefused"]);
});

test("an overnight run's refused message keeps its row, as its sent ones do", () => {
  const refusal: OrchestratorStep = { requestId: "run-1", atMs: 3000, position: 3, kind: { type: "messageRefused", title: "Billing", reason: "Billing is archived." } };
  const sent = record("m1", 4, "run-1", { type: "delivered" });
  const blocks = blocksOf(runThread, peerDigest([sent], [refusal]), runRequests);
  const steps = keyed(blocks, "u1").orchestratorSteps.map((step) => step.kind.type === "peerSent" ? step.kind.messageIds.join(" ") : step.kind.type);
  assert.deepEqual(steps, ["messageRefused", "m1"]);
});

test("an overnight run's usage limit reset keeps its divider (issue 128)", () => {
  const resumed: OrchestratorStep = { requestId: "run-1", atMs: 3000, position: 3, kind: { type: "quotaResumed", provider: "claude" } };
  const blocks = blocksOf(runThread, peerDigest([], [resumed]), runRequests);
  assert.deepEqual(keyed(blocks, "u1").orchestratorSteps.map((step) => step.kind.type), ["quotaResumed"]);
});

test("an overnight run's attached proof reaches its merge message", () => {
  const proof: OrchestratorStep = { requestId: "run-1", atMs: 3000, position: 3, kind: { type: "proof", id: "proof-1", name: "shot.png", mime: "image/png" } };
  const blocks = blocksOf(runThread, peerDigest([], [proof]), runRequests);
  assert.deepEqual(keyed(blocks, "u1").orchestratorSteps.map((step) => step.kind.type), ["proof"]);
});

test("another session's message names its sender; the user's own message is no peer's", () => {
  assert.deepEqual(peerOf(fromPeer("m1", 1, "m1")), { messageId: "m1", from: "api", reply: false, earlier: false });
  assert.deepEqual(peerOf(fromPeer("r1", 5, "a1", "q1"), true), { messageId: "r1", from: "api", reply: true, earlier: true });
  assert.equal(peerOf(said("u", 1, "user", "u")), undefined);
  // A turn's message is the user's.
  const [turn] = blocksOf([said("u", 1, "user", "u")], peerDigest(), requestsOf(peerRequest("u", 1)));
  assert.equal(anchorMessage(turn!)?.id, "u");
});

test("a message steered into a running turn joins it, its request with the turn's", () => {
  const thread = [said("d2", 1, "user", "d2"), said("d2-said", 2, "assistant", "d2"), said("m3", 3, "user", "m3"), said("m3-answer", 4, "assistant", "m3")];
  const steered = requestsOf(peerRequest("d2", 1, { state: { type: "working" } }), peerRequest("m3", 3, { state: { type: "working" }, steeredInto: "d2", steeredAfter: "d2-said" }));
  const blocks = blocksOf(thread, peerDigest(), steered);
  assert.equal(blocks.length, 1);
  const [turn] = blocks;
  assert.deepEqual(turn!.requestIds, ["d2", "m3"]);
  assert.deepEqual(turn!.steers.map((steer) => [steer.message.id, steer.message.role]), [["m3", "user"]]);
  assert.equal(turn!.state, "working");
});

test("a reply a wait took or was handed is held for it; a returned one is not", () => {
  const reply = record("r1", 5, "a1", { type: "seen" }, false);
  for (const hold of [{ type: "handed" }, { type: "taken", itemId: "w1" }] as NonNullable<PeerMessage["hold"]>[]) {
    assert.equal(heldForWait({ ...reply, hold }), true, hold.type);
  }
  assert.equal(heldForWait({ ...reply, hold: { type: "returned" } }), false);
  assert.equal(heldForWait(reply), false);
  assert.equal(heldForWait(undefined), false);
});

test("a block's sequence keeps the anchors on its text", () => {
  const entries = blockSequence({ texts: [{ position: 1, anchors: ["auto1", "auto2"] }], cards: [], steers: [], compactions: [], orchestratorSteps: [], rows: [] });
  assert.deepEqual(entries.map((entry) => entry.kind === "text" && entry.anchors), [["auto1", "auto2"]]);
});

const text = (index: number, position: number): SequenceEntry => ({ kind: "text", index, position });

test("a block still at work answers with the reply its CLI called the final answer, only while that is its newest entry", () => {
  const thinking = { kind: "thinking", segment: {} as never, live: true, position: 9 } as SequenceEntry;
  const texts = [{}, { answerAtMs: 50 }];
  assert.equal(answeringIndex(texts, [text(0, 1), text(1, 3)]), 1);
  // A thought after it doesn't take it back.
  assert.equal(answeringIndex(texts, [text(0, 1), text(1, 3), thinking]), 1);
  // A line between steps, or the answer with work after it, is not the answer yet.
  assert.equal(answeringIndex(texts, [text(1, 3), text(0, 4)]), null);
  assert.equal(answeringIndex(texts, [text(1, 3), { kind: "steer", position: 4, text: "more", atMs: 0, attachments: [] } as SequenceEntry]), null);
  assert.equal(answeringIndex([{}], [text(0, 1)]), null);
});

test("a late update line written while the request still works stays under the answer it follows (#55)", () => {
  const texts = [{}, { answerAtMs: 50 }, { answerAtMs: 90, update: true }];
  const late = { kind: "orchestrator", steps: [], position: 4 } as unknown as SequenceEntry;
  // The answer stays the answer, its time stopping where it started; the update shows under it.
  const answering = answeringIndex(texts, [text(0, 1), text(1, 3), late, text(2, 5)]);
  assert.equal(answering, 1);
  assert.deepEqual(updateIndices(texts, answering), [2]);
  // A block of update lines alone answers with its newest.
  assert.equal(answeringIndex([{ answerAtMs: 1, update: true }], [text(0, 1)]), 0);
  // "**Update**:" is an update line too.
  assert.equal(isUpdate("**Update**: CI passed on c9cb8b3."), true);
});

test("a request that works again after its answer reads as reopened, not as still answering", () => {
  // Answered at 5s while its span ran on (a review it waits for): still the answer's own work.
  assert.equal(reopenedSince([{ fromMs: 1_000, toMs: null }], 5_000), false);
  // Done at 6s, then a late full-checks result made it work again at 9s.
  assert.equal(reopenedSince([{ fromMs: 1_000, toMs: 6_000 }, { fromMs: 9_000, toMs: null }], 5_000), true);
  // Done, and nothing reopened it.
  assert.equal(reopenedSince([{ fromMs: 1_000, toMs: 6_000 }], 5_000), false);
  assert.equal(reopenedSince([{ fromMs: 9_000, toMs: null }], undefined), false);
});

const gapCall = (position: number, atMs: number, endedAtMs: number) => ({
  position,
  atMs,
  kind: { type: "tool" as const, itemId: `c${position}`, name: "Read", detail: "a.ts", status: "completed" as const, throughPosition: position, endedAtMs },
});
const gapKinds = (steps: ReturnType<typeof gapCall>[], texts: { position: number; atMs: number }[]) =>
  blockSequence({ texts, cards: [], steers: [], compactions: [], orchestratorSteps: steps, rows: [] }).map((entry) =>
    entry.kind === "gap" ? `gap@${entry.atMs}` : entry.kind,
  );

test("more than an hour between two things a turn did puts a time line between them, never at its edges", () => {
  const HOUR = 3_600_000;
  const call = gapCall;
  // Two hours after the first call ended, the next one: a line, at the time it came.
  assert.deepEqual(gapKinds([call(1, 0, 1000), call(2, 2 * HOUR, 2 * HOUR + 1000)], [{ position: 3, atMs: 2 * HOUR + 5000 }]), [
    "orchestrator",
    `gap@${2 * HOUR}`,
    "orchestrator",
    "text",
  ]);
  // A call that itself ran two hours says so on its row: no line after it.
  assert.deepEqual(gapKinds([call(1, 0, 2 * HOUR), call(2, 2 * HOUR + 1000, 2 * HOUR + 2000)], []), ["orchestrator", "orchestrator"]);
  // An hour or less: none.
  assert.deepEqual(gapKinds([call(1, 0, 1000), call(2, HOUR, HOUR + 1000)], []), ["orchestrator", "orchestrator"]);
  // The answer written long after the work: the line comes before it.
  assert.deepEqual(gapKinds([call(1, 0, 1000)], [{ position: 2, atMs: 3 * HOUR }]), ["orchestrator", `gap@${3 * HOUR}`, "text"]);
});

test("where a usage limit reset, its divider stands in for the time line, never both (issue 128)", () => {
  const HOUR = 3_600_000;
  const resumed = { position: 2, atMs: 2 * HOUR, kind: { type: "quotaResumed" as const, provider: "claude" as const } };
  const kinds = blockSequence({ texts: [], cards: [], steers: [], compactions: [], orchestratorSteps: [gapCall(1, 0, 1000), resumed, gapCall(3, 2 * HOUR + 1000, 2 * HOUR + 2000)], rows: [] })
    .map((entry) => (entry.kind === "orchestrator" ? (entry.steps[0]?.kind.type ?? "") : entry.kind));
  assert.deepEqual(kinds, ["tool", "quotaResumed", "tool"]);
});

test("a limit or switch of an overnight run keeps its divider, and a finished turn keeps it in view (issue 506)", () => {
  const moment: OrchestratorStep = { requestId: "run-1", atMs: 3000, position: 3, kind: { type: "moment", text: "Claude Code is available again; continuing with it." } };
  const blocks = blocksOf(runThread, peerDigest([], [moment]), runRequests);
  const block = keyed(blocks, "u1");
  assert.deepEqual(block.orchestratorSteps.map((step) => step.kind.type), ["moment"]);
  const kept = sequence(block).filter((entry) => entry.kind === "orchestrator" && !foldsAway(entry, 1));
  assert.equal(kept.length, 1, "never folded away");
});
