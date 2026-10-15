import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { type Activity, groupActivity, type Sorted, thinkingLabel, turnActivity, workerActivity, workerCliNotice } from "@/app/conversation/activity/group";
import { type BlockOrchestratorStep, blockSequence, buildBlocks, type SequenceEntry } from "@/app/conversation/blocks";
import type { ThreadEntry } from "@/components/transcript/activity";
import type { TranscriptItem } from "@/components/transcript/transcript";
import { answerWords, questionAnswers, questionRound, questionRowWords } from "@/app/conversation/cards/questionRound";
import type { EventEnvelope, Message, Question, UserRequest } from "@/ipc/generated";
import { applyToBoard, emptyBoard } from "@/state/board";

type Entry = { step: string } | { thought: string } | { text: string };

// As both threads sort them: thinking is never shown.
const sort = (entry: Entry): Sorted<string> =>
  "step" in entry ? { type: "step", key: entry.step, step: entry.step } : "thought" in entry ? { type: "skip" } : { type: "break" };

/** The activity as letters: `[s1 s2]` a group, `"x"` an entry. */
const shape = (activity: Activity<string, Entry>[]) =>
  activity.map((item) =>
    item.type === "group" ? `[${item.items.map((inner) => inner.step).join(" ")}]` : `"${"text" in item.entry ? item.entry.text : "?"}"`,
  );

test("thinking is never a row and never ends a group", () => {
  assert.deepEqual(
    shape(groupActivity([{ step: "s1" }, { thought: "t1" }, { step: "s2" }, { text: "x" }, { step: "s3" }], sort)),
    ["[s1 s2]", '"x"', "[s3]"],
  );
  // A turn that only thought and delegated has no rows for it.
  assert.deepEqual(shape(groupActivity([{ thought: "t1" }, { text: "x" }, { thought: "t2" }], sort)), ['"x"']);
});

const reasoning = (key: string, text: string, streaming = false): TranscriptItem => ({
  kind: "reasoning",
  key,
  text,
  streaming,
  startedAtMs: 0,
  endedAtMs: 3000,
});
const command = (key: string, line: string): TranscriptItem => ({
  kind: "command",
  key,
  command: line,
  cwd: null,
  status: "completed",
  exitCode: 0,
  output: "",
  durationMs: 10,
});
const tool = (key: string, name: string): TranscriptItem => ({ kind: "tool", key, name, input: null, status: "completed", output: null });
const message = (key: string, text: string): TranscriptItem => ({ kind: "message", key, role: "assistant", text, streaming: false });

test("a worker's thread groups the same way, its plumbing and its thinking hidden", () => {
  const entries: ThreadEntry[] = [
    { kind: "actions", key: "a", items: [command("c1", "cat notes.py") as never, tool("ts", "ToolSearch") as never] },
    { kind: "item", item: reasoning("r1", "Now the tests.") },
    // Answered, an approval doesn't split the work.
    { kind: "item", item: { kind: "approval", key: "ap", resolution: "approved" } as never },
    { kind: "actions", key: "b", items: [command("c2", "pnpm test") as never] },
    { kind: "item", item: message("m1", "Done.") },
    { kind: "item", item: reasoning("r2", "Thinking now", true) },
  ];
  const activity = workerActivity(entries);
  assert.equal(activity.length, 2);
  const [group, reply] = activity;
  assert.equal(group?.type, "group");
  if (group?.type !== "group") return;
  assert.deepEqual(group.items.map((item) => item.key), ["c1", "c2"]);
  assert.equal(reply?.type === "entry" && reply.entry.kind === "item" && reply.entry.item.kind, "message");
});

const recorded = JSON.parse(
  readFileSync(new URL("../../../fixtures/boards/thread-t1-2026-10-08.events.json", import.meta.url), "utf8"),
) as { conversationId: string; events: EventEnvelope[] };

/** T1's recorded session, replayed through the board as the thread renders it. */
function t1Turns() {
  let board = emptyBoard(recorded.conversationId);
  const messages: Message[] = [];
  for (const envelope of recorded.events) {
    board = applyToBoard(board, envelope);
    if (envelope.event.type === "messageAppended") messages.push({ ...envelope.event.message, seq: envelope.streamSeq });
  }
  return buildBlocks(messages, {}, false, board, []).map((block) => turnActivity(blockSequence({ ...block, steers: [] })));
}

test("T1's unfolded turn: one row per run of work, and no thought rows", () => {
  const turns = t1Turns();
  const activity = turns.flat();
  const groups = activity.filter((item) => item.type === "group");
  assert.ok(groups.length > 0, "the turn has work");
  for (const turn of turns) {
    for (const [index, item] of turn.entries()) {
      if (item.type !== "group") continue;
      assert.ok(item.items.length > 0, "no empty group");
      assert.notEqual(turn[index + 1]?.type, "group", "two groups never sit next to each other");
    }
    assert.ok(turn.every((item) => item.type === "group" || item.entry.kind !== "thinking"), "thinking is never an entry");
  }
});

test("the live line says the newest heading, else the last whole line, never half a line", () => {
  assert.equal(thinkingLabel("**Checking the tests**\n\nI should run them first."), "Checking the tests");
  const streamed = "**Reading the code**\n\nSome text.\n\n**Planning the fix**\n\nMore.";
  assert.equal(thinkingLabel(streamed), "Planning the fix");
  // No heading: the last line a newline closed, markdown left out.
  assert.equal(thinkingLabel("Reading `blocks.ts` and the *fold* code.\nThen the te"), "Reading blocks.ts and the fold code.");
  // A line still streaming is never shown: "Thinking" says it meanwhile.
  assert.equal(thinkingLabel("I want to analyze the panel, so I'm th"), "");
  assert.equal(thinkingLabel("**Checking the te"), "");
  // Complete, its last line is whole.
  assert.equal(thinkingLabel("One line only.", true), "One line only.");
  assert.equal(thinkingLabel(""), "");
  for (const text of ["", "line one\nline two\nline three", "**A**\n\nbody\n\n**B**\nmore", "x".repeat(500)]) {
    assert.doesNotMatch(thinkingLabel(text) || "Thinking", /\n/);
  }
});

function notice(key: string, cli?: "started" | "exited"): ThreadEntry {
  const item = { kind: "notice" as const, key, level: cli === "exited" ? ("warning" as const) : ("info" as const), text: key };
  return { kind: "item", item: cli ? { ...item, cli } : item };
}

test("a worker's CLI start and exit are plumbing, unless the worker failed", () => {
  assert.equal(workerCliNotice(notice("Session s1 started", "started"), false), true);
  assert.equal(workerCliNotice(notice("Session s1 started", "started"), true), true);
  // Brigadier ends a landed worker's CLI itself: exit 1 there is no failure.
  assert.equal(workerCliNotice(notice("CLI exited with code 1", "exited"), false), true);
  assert.equal(workerCliNotice(notice("CLI exited with code 1", "exited"), true), false);
  assert.equal(workerCliNotice(notice("Approval a1 answered"), false), false);
});

/** A thread's question card: a round of `count` questions, answered when `answers` is given. */
function roundCard(id: string, requestId: string, position: number, count: number, answers: string[] | null): Question {
  const items = Array.from({ length: count }, (_, index) => ({
    text: `Question ${index + 1}?`,
    options: [{ label: "Yes", description: null }, { label: "No", description: null }],
    recommended: 0,
  }));
  return {
    id,
    conversationId: "c",
    taskId: null,
    requestId,
    position,
    kind: { type: "orchestrator" },
    text: items.map((item) => item.text).join("\n"),
    options: [],
    recommended: null,
    items,
    answer: answers === null ? null : answers.join("\n"),
    answers: answers ?? [],
    createdAtMs: position,
    answeredAtMs: answers === null ? null : position + 1,
  };
}

const doneRequest = (id: string, fields: Partial<UserRequest>): UserRequest => ({
  id,
  conversationId: "c",
  preview: id,
  state: { type: "done" },
  startedAtMs: 0,
  endedAtMs: 100,
  steeredInto: null,
  steeredAfter: null,
  undo: null,
  worked: [{ fromMs: 0, toMs: 100 }],
  quotaWait: false,
  ...fields,
});
const userOf = (id: string, seq: number): Message => ({
  id,
  conversationId: "c",
  seq,
  role: "user",
  text: id,
  blob: null,
  createdAtMs: seq,
  attachments: [],
  mentions: [],
  model: null,
  requestId: id,
  parentId: null,
});
test("an interview is one block: two answered rounds and a steer, each round a row of the folded work", () => {
  const first = roundCard("q1", "grill", 2, 3, ["Yes", "No", "Yes"]);
  const second = roundCard("q2", "grill", 5, 3, ["No", "Yes", "Yes"]);
  const board = {
    ...emptyBoard("c"),
    requests: {
      grill: doneRequest("grill", {}),
      steer: doneRequest("steer", { startedAtMs: 3, steeredInto: "grill", worked: [{ fromMs: 3, toMs: 4 }] }),
    },
    questions: { q1: first, q2: second },
  };
  const blocks = buildBlocks([userOf("grill", 1), userOf("steer", 3)], {}, false, board, []);
  assert.equal(blocks.length, 1);
  const [block] = blocks;
  assert.ok(block);
  assert.deepEqual(block.requestIds, ["grill", "steer"]);
  const sequence = blockSequence({
    ...block,
    steers: block.steers.map((steer) => ({ position: steer.position, text: steer.text, atMs: 0, attachments: [] })),
  });
  const kinds = sequence.map((entry) => (entry.kind === "card" ? `card:${entry.card.id}` : entry.kind));
  assert.deepEqual(kinds, ["card:q1", "steer", "card:q2"]);
  // Neither round stays out of the fold: both fold with the work once it is done.
  assert.ok(block.cards.every((card) => !card.keep));
  assert.equal(turnActivity(sequence).length, 3);
  assert.deepEqual([first, second].map(questionRowWords), ["Asked: Question 1? +2 more → Yes", "Asked: Question 1? +2 more → No"]);
});

test("a question card's row says what it asks, what it asked, and the recommended answer", () => {
  const open = roundCard("q1", "r", 1, 2, null);
  assert.equal(questionRowWords(open), "Asking questions");
  const withdrawn = { ...open, answeredAtMs: 2 };
  assert.equal(questionRowWords(withdrawn), "Asked 2 questions · withdrawn");
  const single = { ...roundCard("q2", "r", 1, 1, ["No"]), items: [], text: "Ship it?", options: ["Yes", "No"], recommended: 0, answers: [], answer: "No" };
  assert.equal(questionRowWords(single), "Asked: Ship it? → No");
  const [item] = questionRound(single);
  assert.ok(item);
  assert.equal(item.text, "Ship it?");
  assert.deepEqual(questionAnswers(single), ["No"]);
  assert.equal(answerWords(item, "Yes"), "Yes (Recommended)");
  assert.equal(answerWords(item, "No"), "No");
  const merge = { ...single, kind: { type: "merge" as const, branch: "b", base: "main", conflicted: false, conflicts: [] }, answer: null, answeredAtMs: null };
  assert.equal(questionRowWords(merge), "Asking whether to merge into main");
});

const stepAt = (position: number, kind: BlockOrchestratorStep["kind"]): SequenceEntry => ({ kind: "orchestrator", steps: [{ kind, position }], position });
const waitCall = (itemId: string) => ({ itemId, status: "completed" as const, requestId: "a1" });

test("messages to other sessions and the waits for their replies, next to each other, are one row of the work", () => {
  const sequence: SequenceEntry[] = [
    stepAt(1, { type: "peerSent", messageIds: ["m1"], waits: [] }),
    stepAt(2, { type: "peerSent", messageIds: ["m2"], waits: [] }),
    stepAt(3, { type: "peerSent", messageIds: [], waits: [waitCall("w1")] }),
    stepAt(4, { type: "tool", itemId: "t1", name: "Read", detail: null, status: "completed", throughPosition: 4 }),
    stepAt(5, { type: "peerSent", messageIds: [], waits: [waitCall("w2")] }),
  ];
  const [group] = turnActivity(sequence);
  assert.ok(group?.type === "group");
  assert.deepEqual(group.items.map((item) => item.type === "step" ? item.step.kind : item.type), [
    { type: "peerSent", messageIds: ["m1", "m2"], waits: [waitCall("w1")] },
    { type: "tool", itemId: "t1", name: "Read", detail: null, status: "completed", throughPosition: 4 },
    { type: "peerSent", messageIds: [], waits: [waitCall("w2")] },
  ]);
});
