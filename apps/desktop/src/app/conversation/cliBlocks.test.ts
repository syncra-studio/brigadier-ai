import assert from "node:assert/strict";
import { test } from "node:test";

import { cliBlocks, cliThreadTree, exitOf, stepDetail, toolItem, workerNotices } from "@/app/conversation/cliBlocks";
import { INTERRUPTED } from "@/app/conversation/turnError";
import { answerIndex, blockSequence, type BoardDigest, foldsAway, seamMove } from "@/app/conversation/blocks";
import { turnActivity } from "@/app/conversation/activity/group";
import { turnParts } from "@/app/conversation/workParts";
import type { FileEntry, Message, ProviderEvent, ThreadTurn, ThreadView } from "@/ipc/generated";

const at = (event: ProviderEvent, atMs: number, more: Partial<FileEntry> = {}): FileEntry => ({ event, atMs, ...more });
const user = (itemId: string, text: string, atMs: number, more: Partial<FileEntry> = {}) =>
  at({ type: "message", itemId, role: "user", text }, atMs, more);
const tool = (itemId: string, name: string, input: string | null, status: "inProgress" | "completed" | "failed", output: string | null = null): ProviderEvent => ({
  type: "toolCall",
  itemId,
  name,
  input,
  status,
  output,
});
const turn = (userMessage: string, startedAtMs: number, endedAtMs: number | null, end: ThreadTurn["end"], error: string | null = null): ThreadTurn => ({
  userMessage,
  startedAtMs,
  endedAtMs,
  end,
  error,
  requests: [],
});

/** A thread with a turn of each ending: done, stopped, failed, interrupted, and one running. */
const view: ThreadView = {
  entries: [
    user("u1", "Fix the test", 1000),
    at({ type: "reasoning", itemId: "r1", text: "Looking at the test first." }, 1100),
    at(tool("t1", "Read", '{"file_path":"/repo/src/a.ts"}', "completed", "1\tcode"), 1200, { updatedAtMs: 3500 }),
    at({ type: "command", itemId: "c1", command: "/bin/zsh -lc 'pnpm test'", cwd: null, status: "completed", exitCode: 0, output: "ok", durationMs: 2000 }, 1300),
    // Brigadier's plumbing: no row.
    at(tool("t2", "ToolSearch", '{"query":"select:Read"}', "completed"), 1350),
    at(tool("agent", "Agent", '{"description":"Look around","prompt":"…"}', "completed", "Found it."), 1400),
    // The subagent's own work: under its Agent row, not drawn.
    at({ type: "command", itemId: "sub-c", command: "ls", cwd: null, status: "completed", exitCode: 0, output: "a", durationMs: 5 }, 1450, { parent: "agent" }),
    user("s1", "Also check the docs", 1500, { steer: true }),
    at({ type: "fileChanges", itemId: "f1", changes: [{ path: "src/a.ts", kind: "update", diff: "-old\n+new\n+more" }], status: "completed" }, 1600),
    at({ type: "message", itemId: "a1", role: "assistant", text: "Fixed: the test passes." }, 1700),

    user("u2", "Run the slow one", 5000),
    at({ type: "command", itemId: "c2", command: "sleep 100", cwd: null, status: "failed", exitCode: null, output: null, durationMs: null }, 5100, { updatedAtMs: 6000 }),

    user("u3", "Try the API", 7000),
    at({ type: "error", error: { kind: "other", message: "API Error: 500", willRetry: false, limit: null, code: null } }, 7400),

    user("u4", "Build it", 8000),
    at(tool("b1", "Bash", '{"command":"cargo build"}', "inProgress"), 8100),

    user("u5", "What next?", 9000),
    at({ type: "reasoningDelta", itemId: "r5", text: "Weighing it" }, 9100, { updatedAtMs: 9150 }),
    at({ type: "messageDelta", itemId: "m5", text: "Checking the plan", phase: "commentary" }, 9200),
  ],
  turns: [
    turn("u1", 1000, 4000, "done"),
    turn("u2", 5000, 6000, "stopped"),
    turn("u3", 7000, 7500, "failed", "API Error: 500"),
    turn("u4", 8000, 8200, "interrupted"),
    turn("u5", 9000, null, null),
  ],
};

test("a thread's turns from its CLI's file are the thread's blocks, one per turn", () => {
  const blocks = cliBlocks(view, "conv");
  assert.deepEqual(blocks.map((block) => [block.key, block.user?.kind === "message" ? block.user.text : null, block.state, block.error]), [
    ["u1", "Fix the test", "done", null],
    ["u2", "Run the slow one", "stopped", null],
    ["u3", "Try the API", "failed", "API Error: 500"],
    ["u4", "Build it", "failed", INTERRUPTED],
    ["u5", "What next?", "working", null],
  ]);

  const [done, stopped, failed, interrupted, running] = blocks;
  assert.ok(done && stopped && failed && interrupted && running);
  // "Worked for …" runs from the CLI's start of the turn to its end.
  assert.deepEqual([done.startedAtMs, done.endedAtMs, done.worked], [1000, 4000, [{ fromMs: 1000, toMs: 4000 }]]);
  assert.deepEqual(done.user?.kind === "message" && done.user.message, {
    id: "u1",
    conversationId: "conv",
    seq: 1,
    role: "user",
    text: "Fix the test",
    blob: null,
    createdAtMs: 1000,
    attachments: [],
    mentions: [],
    model: null,
    requestId: "u1",
    parentId: null,
  } satisfies Message);
  assert.deepEqual(done.thinking, [
    { itemId: "r1", requestId: "u1", text: "Looking at the test first.", position: 1, startedAtMs: 1100, updatedAtMs: 1100, throughPosition: 1, complete: true },
  ]);
  assert.deepEqual(done.texts, [{ messageId: "a1", position: 9, text: "Fixed: the test passes.", model: null, update: false, atMs: 1700 }]);
  assert.deepEqual(
    done.steers.map((steer) => [steer.text, steer.position, steer.message.createdAtMs]),
    [["Also check the docs", 7, 1500]],
  );
  // Rows in the words the engine gives the live stream's calls; the plumbing and the subagent's own work are not rows.
  assert.deepEqual(done.orchestratorSteps, [
    {
      kind: { type: "tool", itemId: "t1", name: "Read", detail: "/repo/src/a.ts", status: "completed", throughPosition: 0, endedAtMs: 3500, item: "tool" },
      position: 2,
      atMs: 1200,
      requestId: "u1",
    },
    {
      kind: { type: "tool", itemId: "c1", name: "shell", detail: "pnpm test", status: "completed", throughPosition: 0, endedAtMs: 3300, exit: 0, item: "command" },
      position: 3,
      atMs: 1300,
      requestId: "u1",
    },
    {
      kind: { type: "tool", itemId: "agent", name: "Agent", detail: null, status: "completed", throughPosition: 0, endedAtMs: 1400, item: "tool" },
      position: 5,
      atMs: 1400,
      requestId: "u1",
    },
    {
      kind: {
        type: "tool",
        itemId: "f1",
        name: "apply_patch",
        detail: "src/a.ts",
        status: "completed",
        throughPosition: 0,
        endedAtMs: 1600,
        item: "files",
        files: [{ path: "src/a.ts", insertions: 2, deletions: 1, binary: false, change: "update" }],
      },
      position: 8,
      atMs: 1600,
      requestId: "u1",
    },
  ]);
  for (const block of blocks) assert.deepEqual([block.cards, block.tasks, block.rows], [[], [], []]);

  // The user's Stop: a call it ended reads stopped.
  assert.deepEqual(stopped.orchestratorSteps.map((step) => step.kind), [
    { type: "tool", itemId: "c2", name: "shell", detail: "sleep 100", status: "failed", stopped: true, throughPosition: 0, endedAtMs: 6000, item: "command" },
  ]);
  assert.deepEqual(stopped.worked, [{ fromMs: 5000, toMs: 6000 }]);
  // The error is the turn's, not a row of its own.
  assert.deepEqual(failed.orchestratorSteps, []);
  // A call the CLI never finished runs no clock.
  assert.deepEqual(interrupted.orchestratorSteps.map((step) => step.kind), [
    { type: "tool", itemId: "b1", name: "Bash", detail: "cargo build", status: "inProgress", throughPosition: 0, item: "command" },
  ]);

  // The running turn works on, its time open, its reasoning still coming and its line between steps streaming.
  assert.deepEqual([running.endedAtMs, running.worked], [null, [{ fromMs: 9000, toMs: null }]]);
  assert.deepEqual(running.thinking.map((segment) => [segment.text, segment.complete, segment.updatedAtMs]), [["Weighing it", false, 9150]]);
  assert.deepEqual(running.texts, [{ messageId: "m5", position: 18, text: "Checking the plan", model: null, update: false, commentary: true, atMs: 9200 }]);
});

test("a turn the stream started, not read again yet, works", () => {
  const blocks = cliBlocks({ entries: [...view.entries.slice(0, 10), user("u9", "Next", 9900)], turns: view.turns.slice(0, 1) }, "conv");
  assert.deepEqual(blocks.map((block) => [block.key, block.state, block.startedAtMs, block.endedAtMs]), [
    ["u1", "done", 1000, 4000],
    ["u9", "working", 9900, null],
  ]);
});

test("a compaction in a turn shows in it, and images in the user's message are its attachments", () => {
  const image = { file: "/s.jsonl", line: 3, index: 0, mime: "image/png" };
  const blocks = cliBlocks(
    {
      entries: [
        user("u1", "Look at this", 1000, { images: [image] }),
        at({ type: "compactionEnded", automatic: true, tokensBefore: 100, tokensAfter: 10, error: null }, 1500),
      ],
      turns: [turn("u1", 1000, 2000, "done")],
    },
    "conv",
  );
  const [block] = blocks;
  assert.deepEqual(block?.compactions, [{ id: "compaction:1", inTurn: true, automatic: true, state: "done", error: null, position: 1, startedAtMs: 1500, endedAtMs: 1500 }]);
  assert.deepEqual(block?.user?.kind === "message" && block.user.message.attachments.map((ref) => [ref.name, ref.mime]), [["Image", "image/png"]]);
});

test("a long session's /compact is never a bubble: its turn shows the compaction's divider", () => {
  const blocks = cliBlocks(
    {
      entries: [
        user("u1", "Grow", 1000),
        user("u2", "/compact Besides your usual summary, keep these exactly", 3000),
        at({ type: "compactionEnded", automatic: false, tokensBefore: 200, tokensAfter: 20, error: null }, 3500),
        user("u3", "After", 5000),
      ],
      turns: [turn("u1", 1000, 2000, "done"), turn("u2", 3000, 4000, "done"), turn("u3", 5000, 6000, "done")],
    },
    "conv",
    undefined,
    true,
  );
  assert.deepEqual(
    blocks.map((block) => [block.key, block.user?.kind ?? null, block.nudges, block.compactions.length]),
    [
      ["u1", "message", [], 0],
      ["u2", null, [], 1],
      ["u3", "message", [], 0],
    ],
  );
});

test("a long session's checkpoint request is never a bubble either", () => {
  const blocks = cliBlocks(
    {
      entries: [
        user("u1", "Fill", 1000),
        user("u2", "[Brigadier · checkpoint] The conversation is about to be compacted.", 3000),
        user("u3", "After", 5000),
      ],
      turns: [turn("u1", 1000, 2000, "done"), turn("u2", 3000, 4000, "done"), turn("u3", 5000, 6000, "done")],
    },
    "conv",
    undefined,
    true,
  );
  assert.deepEqual(
    blocks.map((block) => [block.key, block.user?.kind ?? null, block.nudges]),
    [
      ["u1", "message", []],
      ["u2", null, []],
      ["u3", "message", []],
    ],
  );
});

test("what the user sent shows as ever, /compact and Brigadier's marks too; so does a chat's /compact", () => {
  const entries = [
    user("u1", "/compact keep the API notes", 1000),
    user("u2", "What does [Brigadier · checkpoint] mean?", 3000),
    user("u3", "[Brigadier · checkpoint] pasted from the log", 5000),
  ];
  const asked = (id: string, startedAt: number) => ({ ...turn(id, startedAt, startedAt + 1000, "done"), requests: [`r-${id}`] });
  const sent = cliBlocks({ entries, turns: [asked("u1", 1000), asked("u2", 3000), asked("u3", 5000)] }, "conv", undefined, true);
  assert.deepEqual(
    sent.map((block) => [block.key, block.user?.kind ?? null]),
    [
      ["u1", "message"],
      ["u2", "message"],
      ["u3", "message"],
    ],
  );
  const chat = cliBlocks({ entries: [user("u1", "/compact", 1000)], turns: [turn("u1", 1000, 2000, "done")] }, "conv");
  assert.deepEqual(chat.map((block) => [block.key, block.user?.kind ?? null]), [["u1", "message"]]);
});

test("a background job's end Claude took up shows the CLI's notice line before its reply, in view once the work folds", () => {
  const line = 'Background command "sleep 20 && echo bg-finished" completed (exit code 0)';
  const [block] = cliBlocks(
    {
      entries: [
        user("u1", "Start it in the background", 1000),
        at({ type: "message", itemId: "a1", role: "assistant", text: "Started." }, 1100),
        at({ type: "taskNotice", itemId: "n1", text: line }, 21000),
        at({ type: "message", itemId: "a2", role: "assistant", text: "It finished: bg-finished." }, 21100),
      ],
      turns: [turn("u1", 1000, 21200, "done")],
    },
    "conv",
  );
  assert.ok(block);
  assert.equal(block.user?.kind, "message");
  assert.deepEqual(block.steers, []);
  assert.deepEqual(block.taskNotices, [{ position: 2, text: line }]);
  const sequence = blockSequence({ ...block, steers: [] });
  const notice = sequence.findIndex((entry) => entry.kind === "taskNotice");
  const reply = sequence.findIndex((entry) => entry.kind === "text" && entry.index === 1);
  assert.ok(notice >= 0 && notice < reply, JSON.stringify(sequence));
  // The work folds; the notice is in view between the two answers, the first one kept (#577).
  const parts = turnParts(turnActivity(sequence, false), 1, true, false, () => true);
  assert.deepEqual(
    parts.map((part) => (part.type === "between" ? part.entry.kind : part.answer?.kind === "text" ? `answer ${part.answer.index}` : "work")),
    ["answer 0", "taskNotice"],
  );
});

test("an answer already shown stays in view as its own reply when a background job's end brings another, live and once done", () => {
  const [block] = cliBlocks(
    {
      entries: [
        user("u1", "Run sleep 20 in the background, tell me when it's done", 1000),
        at({ type: "command", itemId: "c1", command: "sleep 20; echo bg-done", cwd: null, status: "completed", exitCode: 0, output: "", durationMs: 5 }, 1100),
        at({ type: "message", itemId: "a1", role: "assistant", text: "It's running in the background now." }, 1200),
        at({ type: "taskNotice", itemId: "n1", text: 'Background command "sleep 20; echo bg-done" completed (exit code 0)' }, 21000),
        at({ type: "message", itemId: "a2", role: "assistant", text: "It finished and printed bg-done." }, 21100),
      ],
      turns: [turn("u1", 1000, 21200, "done")],
    },
    "conv",
  );
  assert.ok(block);
  const sequence = blockSequence({ ...block, steers: [] });
  const activity = turnActivity(sequence, false);
  const answer = answerIndex(block.texts, true);
  assert.equal(answer, 1);
  const [work, notice] = turnParts(activity, answer, true, false, () => true);
  // The first answer is its own reply out of the fold; only the command folds.
  assert.equal(work?.type === "work" && work.answer?.kind === "text" && work.answer.index, 0);
  assert.deepEqual(work?.type === "work" && work.items.map((item) => item.type), ["group"]);
  assert.equal(notice?.type === "between" && notice.entry.kind, "taskNotice");
  assert.equal(foldsAway(sequence.find((entry) => entry.kind === "text" && entry.index === 0)!, answer), true);
});

test("Brigadier's message of what the workers did is a quiet line per worker in plain words, never a user's bubble, as a turn's start and in a turn", () => {
  const news =
    '[worker events wn-b3c2272b85e2] Worker "Fusion opus" (e7c8bc19): stopped · Brigadier restarted while it worked. Send it a message to carry on.; ' +
    'Worker "Fusion codex" (238f8bbe): stopped · Brigadier restarted while it worked. Send it a message to carry on. (ev-4a2c75a9, ev-38ea442f)';
  const lines = [
    'Worker "Fusion opus" stopped · Brigadier restarted while it worked. Send it a message to carry on.',
    'Worker "Fusion codex" stopped · Brigadier restarted while it worked. Send it a message to carry on.',
  ];
  const [first, second] = cliBlocks(
    {
      entries: [
        user("u1", "JSON or SQLite?", 1000),
        at({ type: "message", itemId: "a1", role: "assistant", text: "Asking both models." }, 1100),
        user("n1", news, 5000),
        at({ type: "message", itemId: "a2", role: "assistant", text: "Both were cut off; I'll ask them again." }, 5100),
        user("n2", '[worker events wn-0123456789ab] Worker "" (aa11bb22): finished · Hi. (ev-1234abcd)', 6000, { steer: true }),
      ],
      turns: [turn("u1", 1000, 1200, "done"), turn("n1", 5000, 6200, "done")],
    },
    "conv",
  );
  assert.equal(first?.user?.kind, "message");
  // Its turn has no user message; its lines are the turn's own, without ids.
  assert.equal(second?.user, null);
  assert.deepEqual(second?.steers, []);
  assert.deepEqual(
    second?.taskNotices.map((notice) => notice.text),
    [...lines, "A worker finished · Hi."],
  );
  assert.deepEqual(workerNotices("A message of the user's own"), null);
});

const stored = (id: string, createdAtMs: number): Message => ({
  id,
  conversationId: "conv",
  seq: 1,
  role: "user",
  text: `stored ${id}`,
  blob: null,
  createdAtMs,
  attachments: [],
  mentions: [],
  model: null,
  requestId: id,
  parentId: null,
});
const NO_BOARD: BoardDigest = { tasks: {}, questions: {}, orchestratorSteps: [], machineSteps: [], decisions: [], compactions: {} };

test("the tree chains the turns, then the messages on their way", () => {
  const first = { entries: view.entries.slice(0, 10), turns: [{ ...turn("u1", 1000, 4000, "done"), requests: ["m0"] }] };
  // The CLI has the turn of m0, and m1 is older than what it heard; m2 is on its way, and so is the one being sent.
  const storedNow = [stored("m0", 900), stored("m1", 950), stored("m2", 2000)];
  const pending = [{ localId: "local-1", conversationId: "conv", text: "Sending", createdAtMs: 2300, attachments: [] }];
  const tree = cliThreadTree(first, "conv", storedNow, {}, pending, NO_BOARD, { state: "running", error: null });
  assert.deepEqual(tree.nodes.map((node) => [node.id, node.parentId, node.kind, node.block.state]), [
    ["m0", null, "user", "done"],
    ["request:u1", "m0", "block", "done"],
    ["m2", "request:u1", "user", "working"],
    ["request:m2", "m2", "block", "working"],
    ["local-1", "request:m2", "user", "working"],
    ["request:local-1", "local-1", "block", "working"],
  ]);
  assert.equal(tree.headId, "request:local-1");
  // The turn's user message shows as Brigadier kept it when sent.
  const kept = tree.nodes[0]?.block.user;
  assert.deepEqual(kept?.kind === "message" && [kept.message.id, kept.text], ["m0", "stored m0"]);
  // A run that failed never took it: it reads failed, with the run's words.
  const failed = cliThreadTree(first, "conv", storedNow, {}, [], NO_BOARD, { state: "failed", error: "The CLI did not start." });
  assert.deepEqual(failed.nodes.slice(2).map((node) => [node.id, node.block.state, node.block.error]), [
    ["m2", "failed", "The CLI did not start."],
    ["request:m2", "failed", "The CLI did not start."],
  ]);
});

test("a turn's user message and its steers take the Brigadier requests the turn names", () => {
  const blocks = cliBlocks({ entries: view.entries.slice(0, 10), turns: [{ ...turn("u1", 1000, 4000, "done"), requests: ["r1", "r2"] }] }, "conv");
  const [block] = blocks;
  assert.ok(block);
  assert.deepEqual(block.requestIds, ["r1", "r2"]);
  assert.equal(block.user?.kind === "message" && block.user.message.id, "r1");
  assert.deepEqual(block.steers.map((steer) => steer.message.id), ["r2"]);
});

test("a turn that carries an earlier turn's request keeps its own message, so the tree has no loop", () => {
  // A Codex background terminal that ends after its turn: the note of its end starts a turn of
  // the same request.
  const noted: ThreadView = {
    entries: [
      user("u1", "Start it in the background", 1000),
      at({ type: "message", itemId: "a1", role: "assistant", text: "started" }, 1100),
      user("u2", "[your background command `sleep 60` finished after your turn · exit 0]", 61_000),
      at({ type: "message", itemId: "a2", role: "assistant", text: "It finished." }, 61_100),
    ],
    turns: [
      { ...turn("u1", 1000, 1200, "done"), requests: ["r1"] },
      { ...turn("u2", 61_000, 61_200, "done"), requests: ["r1"] },
    ],
  };
  const tree = cliThreadTree(noted, "conv", [stored("r1", 900)], {}, [], NO_BOARD, { state: "idle", error: null });
  // Brigadier's note of the end is no bubble (run2-D44); the CLI's reply to it shows.
  assert.deepEqual(tree.nodes.map((node) => [node.id, node.parentId]), [
    ["r1", null],
    ["request:u1", "r1"],
    ["request:u2", "request:u1"],
  ]);
  assert.equal(tree.nodes.filter((node) => node.kind === "user").length, 1, "one turn of the user's");
});

const decision = (requestId: string | null, atMs: number) =>
  ({ requestId, atMs, position: 0, kind: { type: "decided", what: `at ${atMs}`, why: "" } }) as unknown as BoardDigest["orchestratorSteps"][number];

test("Brigadier's own items go into the turn of their request, else by time, after the CLI's items before them", () => {
  const two: ThreadView = {
    entries: [user("u1", "One", 1000), at({ type: "message", itemId: "a1", role: "assistant", text: "Done one." }, 1500), user("u2", "Two", 5000), at({ type: "message", itemId: "a2", role: "assistant", text: "Done two." }, 5500)],
    turns: [{ ...turn("u1", 1000, 2000, "done"), requests: ["r1"] }, { ...turn("u2", 5000, 6000, "done"), requests: ["r2"] }],
  };
  // r1's step came late, at 5200, yet it is r1's; the one of no request goes by its time.
  const blocks = cliBlocks(two, "conv", { ...NO_BOARD, orchestratorSteps: [decision("r1", 5200), decision(null, 5600), decision(null, 1200)] });
  assert.deepEqual(
    blocks.map((block) => block.orchestratorSteps.map((step) => [step.kind.type === "decided" ? step.kind.what : step.kind.type, step.position])),
    [
      [["at 5200", 1.5], ["at 1200", 0.5]],
      [["at 5600", 3.5]],
    ],
  );
});

test("a reply the file calls the final answer is the turn's answer from its time; a line between steps is commentary", () => {
  const [block] = cliBlocks(
    {
      entries: [
        user("u1", "Go", 1000),
        at({ type: "message", itemId: "c1", role: "assistant", text: "Looking." }, 1100, { phase: "commentary" }),
        at({ type: "message", itemId: "a1", role: "assistant", text: "Done." }, 1900, { phase: "answer" }),
      ],
      turns: [turn("u1", 1000, 2000, "done")],
    },
    "conv",
  );
  assert.deepEqual(block?.texts.map((text) => [text.messageId, text.commentary ?? false, text.answerAtMs ?? null]), [
    ["c1", true, null],
    ["a1", false, 1900],
  ]);
});

test("a call's kind, detail and exit read as the engine's", () => {
  assert.equal(toolItem("Bash", null), "command");
  assert.equal(toolItem("Read", '{"file_path":"shot.PNG"}'), "image");
  assert.equal(toolItem("WebFetch", null), "web");
  assert.equal(toolItem("Write", null), "files");
  assert.equal(stepDetail('{"action":"findInPage","pattern":"lts","url":"https://nodejs.org"}'), "'lts' in https://nodejs.org");
  assert.equal(stepDetail('{"description":"x"}'), null);
  assert.equal(exitOf("Exit code 2\nboom"), 2);
  assert.equal(exitOf('[{"type":"text","text":"[exit 1] failed"}]'), 1);
  assert.equal(exitOf("fine"), undefined);
});

test("hand-off maintenance prompts and replies stay out of the conversation, with or without seam metadata", () => {
  const seam = { n: 1, open: false, at: "u3", carried: false, nudge: "n1", note: "/s/record/handoff/1.md", start: "/s/record/handoff/1-start.md", death: null, movedTo: null };
  // Steered into the user's turn: the work the CLI finishes first shows; from the ledger's
  // upkeep on, the rest of the turn is the hand-off's.
  const steered: ThreadView = {
    entries: [
      user("u1", "Grow", 1000),
      user("n1", "[Brigadier] A fresh session will continue this work.", 1100, { steer: true }),
      at(tool("t1", "Edit", '{"file_path":"/repo/src/grow.ts"}', "completed"), 1110),
      at({ type: "message", itemId: "a0", role: "assistant", text: "Grow is done." }, 1120),
      at(tool("t2", "Bash", '{"command":"brigadier ledger show --open --rules"}', "completed"), 1130),
      at(tool("t3", "Write", '{"file_path":"/s/abcd/record/handoff/1.md"}', "completed"), 1140),
      at({ type: "message", itemId: "a1", role: "assistant", text: "Handed off." }, 1200),
      user("u3", "After", 2000),
    ],
    turns: [turn("u1", 1000, 1300, "done"), turn("u3", 2000, 2100, "done")],
    seams: [seam],
  };
  const [first, second] = cliBlocks(steered, "conv");
  assert.ok(first && second);
  assert.deepEqual(first.steers, []);
  assert.deepEqual(first.nudges, []);
  assert.deepEqual(first.texts.map((text) => text.text), ["Grow is done."], "the hand-off reply is internal");
  assert.deepEqual(first.orchestratorSteps.map((step) => step.position), [2], "the edit shows, the ledger and the note don't");
  assert.deepEqual(first.seams, [{ n: 1, note: seam.note, start: seam.start, death: null, movedTo: null }]);
  assert.deepEqual(second.seams, []);
  // The thread shows a seam only where it went on in another CLI (S38c), never a hand-off.
  assert.equal(seamMove(first.seams[0]!), null);
  const moved = cliBlocks({ ...steered, seams: [{ ...seam, movedTo: "codex" }] }, "conv");
  assert.equal(seamMove(moved[0]!.seams[0]!), "codex");
  assert.equal(seamMove({ ...moved[0]!.seams[0]!, death: "it never wrote one" }), null);
  // A Codex note of the same: its command and its patch to the note.
  const codex = cliBlocks({ ...steered, entries: [
    ...steered.entries.slice(0, 2),
    at({ type: "command", itemId: "c1", command: "brigadier ledger add decision x", cwd: null, status: "completed", exitCode: 0, output: null, durationMs: 1 }, 1130),
    at({ type: "message", itemId: "a1", role: "assistant", text: "Handed off." }, 1200),
    user("u3", "After", 2000),
  ] }, "conv");
  assert.deepEqual(codex[0]!.texts, []);
  assert.deepEqual(codex[0]!.orchestratorSteps, []);

  // Sent as a turn of its own: that turn has no user bubble; the successor not started yet, the
  // divider ends the thread.
  const own: ThreadView = {
    entries: [
      user("u1", "Big", 1000),
      at({ type: "message", itemId: "a1", role: "assistant", text: "Big." }, 1100),
      user("n1", "[Brigadier] A fresh session will continue this work.", 1200),
      at({ type: "message", itemId: "a2", role: "assistant", text: "Handed off." }, 1300),
    ],
    turns: [turn("u1", 1000, 1150, "done"), turn("n1", 1200, 1400, "done")],
    seams: [{ ...seam, at: null, note: null, death: "it ended its turn without writing one" }],
  };
  const blocks = cliBlocks(own, "conv");
  // A background job's notice the CLI starts a turn with in it keeps the hand-off's turn hidden.
  const noticed = cliBlocks({ ...own, entries: [...own.entries, user("tn", "<task-notification>done</task-notification>", 1350), at({ type: "message", itemId: "a3", role: "assistant", text: "Note written." }, 1360)], turns: [...own.turns, turn("tn", 1350, 1400, "done")] }, "conv");
  assert.deepEqual(noticed.map((block) => block.key), ["u1"]);
  // The user's next turn after it reads its own turn, not the one before: running, its request shown.
  const next = cliBlocks({ ...own, entries: [...own.entries, user("tn", "<task-notification>done</task-notification>", 1350), user("u5", "Next", 1500)], turns: [...own.turns, turn("tn", 1350, 1400, "done"), { ...turn("u5", 1500, null, null), requests: ["r5"] }] }, "conv");
  assert.deepEqual(
    next.map((block) => [block.key, block.user?.kind === "message" ? block.user.message.id : null, block.state]),
    [
      ["u1", "u1", "done"],
      ["u5", "r5", "working"],
    ],
  );
  assert.deepEqual(
    blocks.map((block) => [block.key, block.user?.kind ?? null, block.nudges, block.seams.map((each) => each.death)]),
    [
      ["u1", "message", [], []],
    ],
  );
  // A successor that carried on by itself: its first turn has no user bubble, under the divider.
  const carried = cliBlocks(
    {
      ...steered,
      entries: [...steered.entries.slice(0, 3), user("u3", "Carry on from the hand-off note.", 2000)],
      seams: [{ ...seam, carried: true }],
    },
    "conv",
  );
  assert.deepEqual(
    carried.map((block) => [block.key, block.user?.kind ?? null, block.nudges, block.seams.length]),
    [
      ["u1", "message", [], 1],
      ["u3", null, [], 0],
    ],
  );
  const withoutMetadata = cliBlocks({ ...own, seams: [] }, "conv");
  assert.deepEqual(withoutMetadata.map((block) => block.key), ["u1"]);
  assert.deepEqual(withoutMetadata[0]?.texts.map((text) => text.text), ["Big."]);
  const rawStart = cliBlocks({ entries: [user("start", "\n\n[Brigadier hand-off 2 · conv]\n[End of the hand-off] quoted inside history\n" + "internal\n".repeat(9000) + "[End of the hand-off]\n\nCarry on from the hand-off note.", 2000)], turns: [turn("start", 2000, null, null)] }, "conv");
  assert.equal(rawStart[0]?.user, null, "the raw bootstrap never becomes a chat bubble");
  // The user's own words are never taken for Brigadier's: a turn that carries their request.
  const mine = (text: string) => cliBlocks({ entries: [user("m", text, 3000)], turns: [{ ...turn("m", 3000, 3100, "done"), requests: ["r9"] }] }, "conv")[0]?.user;
  assert.equal(mine("Carry on from the hand-off note.")?.kind, "message");
  assert.equal(mine("[Brigadier hand-off 3 · x] what does this header mean?")?.kind, "message");
  // While the thread writes its note, no divider yet.
  assert.deepEqual(cliBlocks({ ...own, seams: [{ ...seam, open: true, at: null }] }, "conv").flatMap((block) => block.seams), []);
});

test("Brigadier's own words to the CLI are no bubble, as a turn's start or a steer; the CLI's reply shows (run2-D44)", () => {
  const from = "c-0123456789abcdef";
  const sent = (id: string, text: string) => `[message from session "API work" (${from}) · ${id}]\n${text}`;
  const peers: ThreadView = {
    entries: [
      user("u1", sent("sm-aaaaaaaaaaaa", "Can you check the schema?"), 1000),
      user("s1", sent("sm-bbbbbbbbbbbb", "Also the docs."), 1100, { steer: true }),
      at({ type: "message", itemId: "a1", role: "assistant", text: "Checked." }, 1200),
      user("u2", "My own words", 2000),
      user("d1", "[decision] The user approved: run the migration", 3000),
      at({ type: "message", itemId: "a2", role: "assistant", text: "Migrated." }, 3100),
      user("q1", "[answer] You asked the user:\nYes\n[card c1]", 4000),
      user("r1", "[The user stopped you, then asked you to resume. Continue their request from where you left off.]", 5000),
      user("p1", "[preview ended: failed] p1 \"web\" stopped on its own; it is not running now.", 6000),
    ],
    turns: [
      turn("u1", 1000, 1500, "done"),
      { ...turn("u2", 2000, 2500, "done"), requests: ["r-u2"] },
      turn("d1", 3000, 3200, "done"),
      turn("q1", 4000, 4100, "done"),
      turn("r1", 5000, 5100, "done"),
      turn("p1", 6000, 6100, "done"),
    ],
  };
  const blocks = cliBlocks(peers, "c1", NO_BOARD);
  assert.deepEqual(blocks.map((block) => [block.key, block.user?.kind ?? null]), [
    ["u1", null], ["u2", "message"], ["d1", null], ["q1", null], ["r1", null], ["p1", null],
  ]);
  assert.deepEqual(blocks[0]!.steers, []);
  assert.deepEqual(blocks[0]!.texts.map((text) => text.text), ["Checked."]);
  assert.equal(blocks[1]!.user?.kind === "message" && blocks[1]!.user.text, "My own words");
  assert.deepEqual(blocks[2]!.texts.map((text) => text.text), ["Migrated."]);
});

test("a queued message shows once, as the user's, its frame gone: in the rail while the CLI has it there, else where the CLI last took it", () => {
  const id = "0192aaaa-bbbb-7ccc-8ddd-0000feedbeef";
  const deciding = `[queued message feedbeef · judge it: brigadier queue feedbeef join|wait|worker <id>|new-session; unjudged, it joins this work]\nAlso the docs.`;
  const steered: ThreadView = {
    entries: [
      user("u1", "Fix the test", 1000),
      user("s1", "A plain steer", 1050, { steer: true }),
      user("s2", deciding, 1100, { steer: true }),
      at({ type: "message", itemId: "a1", role: "assistant", text: "Fixed." }, 1200),
    ],
    turns: [{ ...turn("u1", 1000, 1500, "done"), requests: ["r1", id, "r3"] }],
  };
  // Joined: its request is its own id; the plain steer takes the turn's other request.
  const [joined] = cliBlocks(steered, "c1", NO_BOARD);
  assert.deepEqual(joined!.steers.map((steer) => [steer.message.id, steer.text]), [["r3", "A plain steer"], [id, "Also the docs."]]);
  // Still in the rail: only the rail shows it.
  const queued = { id, text: "Also the docs.", attachments: [], mentions: [], queuedAtMs: 1100, editedAtMs: null, deciding: true, delivered: true };
  const [inRail] = cliBlocks(steered, "c1", { ...NO_BOARD, queueItems: [queued] });
  assert.deepEqual(inRail!.steers.map((steer) => steer.text), ["A plain steer"]);
  assert.deepEqual(inRail!.nudges, []);
  // Waited for its own turn: drawn there, not where the CLI read it first.
  const waited: ThreadView = {
    entries: [...steered.entries, user("u2", "[queued message feedbeef, now on its own turn]\nAlso the docs.", 2000)],
    turns: [{ ...steered.turns[0]!, requests: ["r1", "r3"] }, { ...turn("u2", 2000, 2500, "done"), requests: [id] }],
  };
  const [first, second] = cliBlocks(waited, "c1", NO_BOARD);
  assert.deepEqual(first!.steers.map((steer) => steer.text), ["A plain steer"]);
  assert.equal(second!.user?.kind === "message" && second!.user.text, "Also the docs.");
  assert.equal(second!.user?.kind === "message" && second!.user.message.id, id);
});

test("Brigadier's word that the user took a queued message back is never drawn", () => {
  const withdrawn = "[queued message feedbeef withdrawn: the user took it back; leave it]";
  const [block] = cliBlocks(
    {
      entries: [user("u1", "Fix the test", 1000), user("s1", withdrawn, 1100, { steer: true }), user("s2", `Then the docs.\n${withdrawn}`, 1200, { steer: true })],
      turns: [turn("u1", 1000, 1500, "done")],
    },
    "c1",
    NO_BOARD,
  );
  assert.deepEqual(block!.steers.map((steer) => steer.text), ["Then the docs."]);
  assert.deepEqual(block!.nudges, []);
});
