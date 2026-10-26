import assert from "node:assert/strict";
import { test } from "node:test";

import { cliBlocks, cliThreadTree, exitOf, stepDetail, stepInTurn, toolItem, wakeWords, workerNotices } from "@/app/conversation/cliBlocks";
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
    at({ type: "messageDelta", itemId: "m5", text: "Checking the plan\nand th", phase: "commentary" }, 9200),
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
      kind: { type: "tool", itemId: "agent", name: "Agent", detail: "Look around", status: "completed", throughPosition: 0, endedAtMs: 1400, item: "tool" },
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
  // A call the CLI never finished was cut off with its turn: it reads stopped when the turn ended (D44).
  assert.deepEqual(interrupted.orchestratorSteps.map((step) => step.kind), [
    { type: "tool", itemId: "b1", name: "Bash", detail: "cargo build", status: "failed", stopped: true, throughPosition: 0, item: "command", endedAtMs: 8200 },
  ]);

  // The running turn works on, its time open, its reasoning still coming and its line between steps streaming.
  assert.deepEqual([running.endedAtMs, running.worked], [null, [{ fromMs: 9000, toMs: null }]]);
  assert.deepEqual(running.thinking.map((segment) => [segment.text, segment.complete, segment.updatedAtMs]), [["Weighing it", false, 9150]]);
  assert.deepEqual(running.texts, [{ messageId: "m5", position: 18, text: "Checking the plan\n", model: null, update: false, commentary: true, atMs: 9200 }]);
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
        at({ type: "message", itemId: "a1", role: "assistant", text: "Looked." }, 1800),
      ],
      turns: [turn("u1", 1000, 2000, "done")],
    },
    "conv",
  );
  const [block] = blocks;
  assert.deepEqual(block?.compactions, [{ id: "compaction:1", inTurn: true, automatic: true, state: "done", error: null, position: 1, startedAtMs: 1500, endedAtMs: 1500 }]);
  assert.deepEqual(block?.user?.kind === "message" && block.user.message.attachments.map((ref) => [ref.name, ref.mime]), [["Image", "image/png"]]);
});

test("a long session's /compact is never a bubble nor a turn: its compaction is a divider after the block before", () => {
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
    blocks.map((block) => [block.key, block.user?.kind ?? null, block.nudges, block.compactions.map((each) => each.inTurn)]),
    [
      ["u1", "message", [], [false]],
      ["u3", "message", [], []],
    ],
  );
});

test("a long session's checkpoint request is never a bubble nor a turn either", () => {
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
      ["u3", "message", []],
    ],
  );
});

const compactionStarted = (automatic: boolean, atMs: number) => at({ type: "compactionStarted", automatic }, atMs);
const compactionEnded = (automatic: boolean, atMs: number, error: string | null = null) =>
  at({ type: "compactionEnded", automatic, tokensBefore: 900, tokensAfter: 40, error }, atMs);

test("Brigadier's /compact as the CLI writes it, the compaction before the /compact, is one divider after the answer, not its time, nor a block", () => {
  const blocks = cliBlocks(
    {
      entries: [
        user("u1", "Is it rotated?", 1000),
        at({ type: "message", itemId: "a1", role: "assistant", text: "Half done." }, 4000),
        compactionEnded(false, 73_000),
        user("c1", "/compact Besides your usual summary, keep these exactly", 5000),
      ],
      // A thread noted before the fix: the /compact turn, the last, read as cut off.
      turns: [turn("u1", 1000, 5000, "done"), { ...turn("c1", 5000, 5000, "interrupted"), error: null }],
    },
    "conv",
    undefined,
    true,
  );
  assert.deepEqual(
    blocks.map((block) => [block.key, block.state, block.endedAtMs, block.compactions.map((each) => [each.inTurn, each.state])]),
    [["u1", "done", 5000, [[false, "done"]]]],
  );
});

const tidying = (entries: ReturnType<typeof at>[], end: "done" | null) => ({ entries: [user("u1", "Tidy it", 1000), ...entries], turns: [turn("u1", 1000, 9000, end)] });
const reply = (id: string, atMs: number) => at({ type: "message", itemId: id, role: "assistant", text: id }, atMs);

test("a running compaction is the divider its end takes over, in place: one, never two", () => {
  // Mid-turn, the CLI's own: in the work while it runs, the same divider once it ends.
  const [running] = cliBlocks(tidying([reply("a1", 2000), compactionStarted(true, 3000)], null), "conv", undefined, true);
  assert.deepEqual(running?.compactions.map((each) => [each.inTurn, each.state, each.position]), [[true, "running", 2]]);
  const [ended] = cliBlocks(tidying([reply("a1", 2000), compactionStarted(true, 3000), compactionEnded(true, 8000), reply("a2", 8500)], null), "conv", undefined, true);
  assert.deepEqual(ended?.compactions.map((each) => [each.inTurn, each.state, each.position, each.startedAtMs, each.endedAtMs]), [[true, "done", 2, 3000, 8000]]);
  // Brigadier's at a turn's end: after the block while it runs, and after it once done; the
  // file's word of the same end, read while the turn is open, is no second divider.
  const [asked] = cliBlocks(tidying([reply("a1", 2000), compactionStarted(false, 9500)], "done"), "conv", undefined, true);
  assert.deepEqual(asked?.compactions.map((each) => [each.inTurn, each.state]), [[false, "running"]]);
  const [twice] = cliBlocks(tidying([reply("a1", 2000), compactionEnded(false, 9800), compactionStarted(false, 9500), compactionEnded(false, 9800)], "done"), "conv", undefined, true);
  assert.deepEqual(twice?.compactions.map((each) => [each.inTurn, each.state]), [[false, "done"]]);
  // A failed one says so.
  const [failed] = cliBlocks(tidying([reply("a1", 2000), compactionEnded(false, 9800, "Not enough messages to compact.")], "done"), "conv", undefined, true);
  assert.deepEqual(failed?.compactions.map((each) => [each.state, each.error]), [["failed", "Not enough messages to compact."]]);
});

test("a Chat's compaction the board and its CLI both say is one divider", () => {
  const board = {
    ...NO_BOARD,
    compactions: {
      k1: { id: "k1", requestId: null, after: null, automatic: false, state: { type: "done" }, tokensBefore: 900, tokensAfter: 40, startedAtMs: 3000, endedAtMs: 8000, position: 5 },
    },
  } as unknown as BoardDigest;
  const [block] = cliBlocks(
    { entries: [user("u1", "Grow", 1000), at({ type: "message", itemId: "a1", role: "assistant", text: "Grown." }, 2000), compactionEnded(false, 7900)], turns: [turn("u1", 1000, 2500, "done")] },
    "conv",
    board,
  );
  assert.equal(block?.compactions.length, 1);
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

test("a background job's end Claude took up is one plain line in the same work, live and after a restart (D21)", () => {
  const thread = {
    entries: [
      user("u1", "Start it in the background", 1000),
      at({ type: "message", itemId: "a1", role: "assistant", text: "Started." }, 1100),
      at({ type: "taskNotice", itemId: "n1", text: 'Background command "sleep 20 && echo bg-finished" completed (exit code 0)' }, 21000),
      at({ type: "message", itemId: "a2", role: "assistant", text: "It finished: bg-finished." }, 21100),
    ],
    turns: [turn("u1", 1000, 21200, "done")],
  };
  const blocks = cliBlocks(thread, "conv");
  assert.equal(blocks.length, 1);
  const [block] = blocks;
  assert.ok(block);
  assert.equal(block.user?.kind, "message");
  // No bubble; one plain line between the two replies, the CLI's own words on hover.
  assert.deepEqual(block.steers, []);
  assert.deepEqual(block.taskNotices, [{
    position: 2,
    id: "n1",
    text: "Background command finished · exit 0",
    detail: 'Background command "sleep 20 && echo bg-finished" completed (exit code 0)',
    inWork: true,
  }]);
  const sequence = blockSequence({ ...block, steers: [] });
  assert.deepEqual(sequence.map((entry) => entry.kind), ["text", "taskNotice", "text"]);
  // Done, the last reply is the answer and the one before it folds into the work with the line, with no part cut out.
  const answer = answerIndex(block.texts, true);
  assert.equal(answer, 1);
  const parts = turnParts(turnActivity(sequence, false), answer, true, false, () => true);
  assert.deepEqual(parts.map((part) => part.type), ["work"]);
  assert.equal(parts[0]?.type === "work" && parts[0].answer, undefined);
  assert.equal(foldsAway(sequence[0]!, answer), true);
});

test("a background job's end Claude took up mid-work keeps the work one run, its command, line and first reply folded", () => {
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
  assert.deepEqual(sequence.filter((entry) => entry.kind === "taskNotice").map((entry) => entry.kind === "taskNotice" && entry.text), ["Background command finished · exit 0"]);
  const answer = answerIndex(block.texts, true);
  assert.equal(answer, 1);
  const parts = turnParts(turnActivity(sequence, false), answer, true, false, () => true);
  assert.equal(parts.length, 1);
  assert.equal(parts[0]?.type === "work" && parts[0].answer, undefined);
});

test("the CLI's words of a background job's end read in a few plain ones (D21)", () => {
  for (const [said, words] of [
    ['Background command "pnpm test" completed (exit code 0)', "Background command finished · exit 0"],
    ['Background command "pnpm test" completed (exit code 2)', "Background command failed · exit 2"],
    ['Background command "pnpm test" failed with exit code 1', "Background command failed · exit 1"],
    ['Background command "gh run watch 12 --exit-status" was stopped', "Background command stopped"],
    ['Background command "sleep 5" was killed', "Background command stopped"],
    ['Monitor "CI run 12" completed', "Monitor finished"],
    ["[your background command `gh run watch 12` finished after your turn · exit 0]\nok", "Background command finished · exit 0"],
    ["[your background command `pnpm test` finished after your turn · exit 1]\nFAIL", "Background command failed · exit 1"],
    ["[your background command `pnpm test` finished after your turn · completed]\n", "Background command finished"],
    ["Something new the CLI says", "Something new the CLI says"],
  ] as const) assert.equal(wakeWords(said), words, said);
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

/** A worker of the session's, done with its run. */
const row = (id: string, startedAtMs: number) => ({
  id, number: 1, title: id, agent: "claude" as const, model: null, startedAtMs, working: false, lastEnd: "done" as const, gone: false,
  lastEventAtMs: null, limit: null, freshContext: false, waiting: [], mcp: [], branch: null, base: null, diff: null, ahead: 0, merged: false,
});

test("Brigadier's word of what workers did joins the block that started them, as one more turn of it; a message the user typed still starts its own", () => {
  const news = (id: string, worker: string, atMs: number) => user(id, `[worker events wn-0123456789ab] Worker "Fix" (${worker}): finished · Done. (ev-1234abcd)`, atMs);
  const blocks = cliBlocks(
    {
      entries: [
        user("u1", "Fix it with a worker", 1000),
        at({ type: "message", itemId: "a1", role: "assistant", text: "Started a worker." }, 1100),
        news("n1", "aa11bb22", 5000),
        at({ type: "message", itemId: "a2", role: "assistant", text: "The worker fixed it." }, 5100),
        user("u2", "Thanks, now the docs", 8000),
        at({ type: "message", itemId: "a3", role: "assistant", text: "On it." }, 8100),
        // A worker the block before started: this block is the user's, so its word is a turn of its own.
        news("n2", "cc33dd44", 9000),
        at({ type: "message", itemId: "a4", role: "assistant", text: "Late news." }, 9100),
      ],
      turns: [turn("u1", 1000, 1200, "done"), turn("n1", 5000, 5200, "done"), turn("u2", 8000, 8200, "done"), turn("n2", 9000, 9200, "done")],
      workers: [row("aa11bb22", 1050), row("cc33dd44", 1060)],
    },
    "conv",
  );
  assert.deepEqual(blocks.map((block) => [block.key, block.user?.kind ?? null]), [["u1", "message"], ["u2", "message"], ["n2", null]]);
  const [first] = blocks;
  assert.ok(first);
  assert.deepEqual(first.texts.map((text) => text.messageId), ["a1", "a2"]);
  // It started with its first turn and worked in each; it ended with the last.
  assert.equal(first.startedAtMs, 1000);
  assert.equal(first.endedAtMs, 5200);
  assert.deepEqual(first.worked, [{ fromMs: 1000, toMs: 1200 }, { fromMs: 5000, toMs: 5200 }]);
  // The worker's line is in the work: done, it folds with it, under one answer.
  assert.deepEqual(first.taskNotices, [{ position: 2, id: "n1:0", text: 'Worker "Fix" finished · Done.', inWork: true }]);
  const sequence = blockSequence({ ...first, steers: [] });
  const answer = answerIndex(first.texts, true);
  assert.equal(answer, 1);
  const parts = turnParts(turnActivity(sequence, false), answer, true, false, () => true);
  assert.deepEqual(parts.map((part) => part.type), ["work"]);
  assert.equal(parts[0]?.type === "work" && parts[0].answer, undefined);
  // The one that stays its own turn keeps its line out of the fold, as before.
  assert.equal(blocks[2]?.taskNotices[0]?.inWork, undefined);
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

/** A tree's nodes as drawn: each user bubble's text, each block's requests. */
const bubbles = (tree: ReturnType<typeof cliThreadTree>) =>
  tree.nodes.map((node) => [node.id, node.kind, node.kind === "user" && node.block.user?.kind === "message" ? node.block.user.text : node.block.requestIds]);

test("a sent message keeps its bubble when the CLI says it before the view reads its turns again", () => {
  const before = { entries: view.entries.slice(0, 10), turns: [{ ...turn("u1", 1000, 4000, "done"), requests: ["m0"] }] };
  const sent = { ...stored("m1", 5000), text: "Next" };
  const kept = [stored("m0", 900), sent];
  const said = user("cli-m1", "Next", 5100);
  const run = { state: "running", error: null } as const;
  // On its way, then said by the CLI with its turn not read yet, then read: one bubble, one id.
  const onItsWay = bubbles(cliThreadTree(before, "conv", kept, {}, [], NO_BOARD, run));
  const unread = bubbles(cliThreadTree({ ...before, entries: [...before.entries, said] }, "conv", kept, {}, [], NO_BOARD, run));
  const read = bubbles(
    cliThreadTree({ entries: [...before.entries, said], turns: [...before.turns, { ...turn("cli-m1", 5100, null, null), requests: ["m1"] }] }, "conv", kept, {}, [], NO_BOARD, run),
  );
  assert.deepEqual(onItsWay.slice(2), [["m1", "user", "Next"], ["request:m1", "block", ["m1"]]]);
  assert.deepEqual(unread.slice(2), [["m1", "user", "Next"], ["request:cli-m1", "block", ["m1"]]]);
  assert.deepEqual(read.slice(2), unread.slice(2));
});

test("a turn's user message and its steers take the Brigadier requests the turn names", () => {
  const blocks = cliBlocks({ entries: view.entries.slice(0, 10), turns: [{ ...turn("u1", 1000, 4000, "done"), requests: ["r1", "r2"] }] }, "conv");
  const [block] = blocks;
  assert.ok(block);
  assert.deepEqual(block.requestIds, ["r1", "r2"]);
  assert.equal(block.user?.kind === "message" && block.user.message.id, "r1");
  assert.deepEqual(block.steers.map((steer) => steer.message.id), ["r2"]);
});

test("a Codex command's end after its turn is one plain line, a turn more of the block that started it, like Claude's (D21)", () => {
  // A Codex background terminal that ends after its turn: Brigadier's note of its end starts a
  // turn of the same request.
  const woke = "[your background command `sleep 60` finished after your turn · exit 0]\nxyz";
  const noted: ThreadView = {
    entries: [
      user("u1", "Start it in the background", 1000),
      at({ type: "message", itemId: "a1", role: "assistant", text: "started" }, 1100),
      user("u2", woke, 61_000),
      at({ type: "message", itemId: "a2", role: "assistant", text: "It finished." }, 61_100),
    ],
    turns: [
      { ...turn("u1", 1000, 1200, "done"), requests: ["r1"] },
      { ...turn("u2", 61_000, 61_200, "done"), requests: ["r1"] },
    ],
  };
  const tree = cliThreadTree(noted, "conv", [stored("r1", 900)], {}, [], NO_BOARD, { state: "idle", error: null });
  // Brigadier's note of the end is no bubble (run2-D44), nor a block of its own; the CLI's reply to it shows.
  assert.deepEqual(tree.nodes.map((node) => [node.id, node.parentId]), [
    ["r1", null],
    ["request:u1", "r1"],
  ]);
  const blocks = cliBlocks(noted, "conv");
  assert.equal(blocks.length, 1);
  const [block] = blocks;
  assert.ok(block);
  assert.deepEqual(block.texts.map((text) => text.messageId), ["a1", "a2"]);
  assert.deepEqual(block.worked, [{ fromMs: 1000, toMs: 1200 }, { fromMs: 61_000, toMs: 61_200 }]);
  assert.deepEqual(block.taskNotices, [{ position: 2, id: "u2", text: "Background command finished · exit 0", detail: woke, inWork: true }]);
  const sequence = blockSequence({ ...block, steers: [] });
  assert.deepEqual(sequence.map((entry) => entry.kind), ["text", "taskNotice", "text"]);
  const answer = answerIndex(block.texts, true);
  const parts = turnParts(turnActivity(sequence, false), answer, true, false, () => true);
  assert.deepEqual(parts.map((part) => part.type), ["work"]);
});

test("a Codex command's end after a later message of the user's stays a turn of its own, with its line, so the tree has no loop", () => {
  const woke = "[your background command `sleep 60` finished after your turn · exit 1]\nFAIL";
  const noted: ThreadView = {
    entries: [
      user("u1", "Start it in the background", 1000),
      at({ type: "message", itemId: "a1", role: "assistant", text: "started" }, 1100),
      user("u2", "And the docs", 5000),
      at({ type: "message", itemId: "a2", role: "assistant", text: "Done." }, 5100),
      user("u3", woke, 61_000),
      at({ type: "message", itemId: "a3", role: "assistant", text: "It failed." }, 61_100),
    ],
    turns: [
      { ...turn("u1", 1000, 1200, "done"), requests: ["r1"] },
      { ...turn("u2", 5000, 5200, "done"), requests: ["r2"] },
      { ...turn("u3", 61_000, 61_200, "done"), requests: ["r1"] },
    ],
  };
  const tree = cliThreadTree(noted, "conv", [stored("r1", 900), stored("r2", 4900)], {}, [], NO_BOARD, { state: "idle", error: null });
  assert.deepEqual(tree.nodes.filter((node) => node.kind === "user").map((node) => node.id), ["r1", "r2"]);
  const blocks = cliBlocks(noted, "conv");
  assert.deepEqual(blocks.map((block) => [block.key, block.user?.kind ?? null]), [["u1", "message"], ["u2", "message"], ["u3", null]]);
  assert.deepEqual(blocks[2]?.taskNotices, [{ position: 4, id: "u3", text: "Background command failed · exit 1", detail: woke, inWork: true }]);
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
      // A turn a restart cut off, going on by itself (D9 a), and one the user continued.
      user("g1", "[Brigadier restarted while you were working on this request, and your turn goes on by itself now. Check git status and git log first.]", 7000),
      user("c1", "[Brigadier quit while you were working on this request, and the user asked you to continue. Check git status and git log first.]", 8000),
    ],
    turns: [
      turn("u1", 1000, 1500, "done"),
      { ...turn("u2", 2000, 2500, "done"), requests: ["r-u2"] },
      turn("d1", 3000, 3200, "done"),
      turn("q1", 4000, 4100, "done"),
      turn("r1", 5000, 5100, "done"),
      turn("p1", 6000, 6100, "done"),
      turn("g1", 7000, 7100, "done"),
      turn("c1", 8000, 8100, "done"),
    ],
  };
  const blocks = cliBlocks(peers, "c1", NO_BOARD);
  assert.deepEqual(blocks.map((block) => [block.key, block.user?.kind ?? null]), [
    ["u1", null], ["u2", "message"], ["d1", null], ["q1", null], ["r1", null], ["p1", null], ["g1", null], ["c1", null],
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

const message = (requestId: string, text: string, createdAtMs: number): Message => ({ ...stored(requestId, createdAtMs), text });
/** A tree's nodes as drawn: each user bubble's text, each block's steers. */
const shown = (tree: ReturnType<typeof cliThreadTree>) =>
  tree.nodes.map((node) => [node.id, node.kind, node.kind === "user" && node.block.user?.kind === "message" ? node.block.user.text : node.block.steers.map((steer) => steer.text)]);

test("a queued message the CLI began its own turn with shows once, heading that turn, live, once joined and after the turn", () => {
  // Session 82123c64's shape: "sure, do" (…2f334bc5) was steered into the turn before; the
  // next message came queued as that turn ended, and the CLI began a turn with it, under its
  // frame. The daemon kept it, as its request, only when the session joined it 5.6s later; the
  // live turn still named only the request before it.
  const before = "01a171d2-7c9f-7787-bd17-63a32f334bc5";
  const id = "01a171d7-b9e5-7250-bc11-f6a7d9f90994";
  const words = "Another issue is why does it keep giving me these questions one by one?";
  const entries = [
    user("07de44de", "Another issue is that in another session, i asked it to start the dev app", 10_000),
    user("65ae8f04", "[queued message 2f334bc5 · judge it: brigadier queue 2f334bc5 join|wait|worker <id>|new-session; unjudged, it joins this work]\nsure, do", 69_000, { steer: true }),
    at({ type: "message", itemId: "a1", role: "assistant", text: "Pushed." }, 412_000),
    user("5cf7b32d", `[queued message d9f90994 · judge it: brigadier queue d9f90994 join|wait|worker <id>|new-session; unjudged, it joins this work]\n${words}`, 412_300),
    at({ type: "command", itemId: "c1", command: "brigadier queue d9f90994 join", cwd: null, status: "completed", exitCode: 0, output: "", durationMs: 40 }, 417_900),
  ];
  const live: ThreadView = {
    entries,
    turns: [{ ...turn("07de44de", 10_000, 411_900, "done"), requests: ["01a171d1-959b-72d2-8538-5e80be977c2b"] }, { ...turn("5cf7b32d", 412_300, null, null), requests: [before] }],
  };
  const keptBefore = [message("01a171d1-959b-72d2-8538-5e80be977c2b", "Another issue is that in another session, i asked it to start the dev app", 9_900), message(before, "sure, do", 68_900)];
  const queued = { id, text: words, attachments: [], mentions: [], queuedAtMs: 411_990, editedAtMs: null, deciding: true, delivered: true };
  const running = { state: "running", error: null } as const;

  // Still judged: the rail has it, the turn it began shows no bubble.
  const judging = cliThreadTree(live, "conv", keptBefore, {}, [], { ...NO_BOARD, queueItems: [queued] }, running);
  assert.deepEqual(shown(judging), [
    ["01a171d1-959b-72d2-8538-5e80be977c2b", "user", "Another issue is that in another session, i asked it to start the dev app"],
    ["request:07de44de", "block", ["sure, do"]],
    ["request:5cf7b32d", "block", []],
  ]);

  // Joined, the turn still running: once, heading the turn it began, never on its way as well.
  const kept = [...keptBefore, message(id, words, 417_936)];
  const joined = cliThreadTree(live, "conv", kept, {}, [], NO_BOARD, running);
  const once = [
    ["01a171d1-959b-72d2-8538-5e80be977c2b", "user", "Another issue is that in another session, i asked it to start the dev app"],
    ["request:07de44de", "block", ["sure, do"]],
    [id, "user", words],
    ["request:5cf7b32d", "block", []],
  ];
  assert.deepEqual(shown(joined), once);
  assert.equal(joined.headId, "request:5cf7b32d");

  // Over, its record naming the request before first (and after a restart, read back so): the same.
  const over: ThreadView = { entries, turns: [live.turns[0]!, { ...turn("5cf7b32d", 412_300, 479_000, "done"), requests: [before, id] }] };
  assert.deepEqual(shown(cliThreadTree(over, "conv", kept, {}, [], NO_BOARD, { state: "idle", error: null })), once);
  // Its frame never shows, even before Brigadier's copy of it is read.
  const [, bare] = cliBlocks(over, "conv", NO_BOARD);
  assert.equal(bare!.user?.kind === "message" && bare!.user.text, words);
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

test("a background command whose task completed reads completed, even when its item never said it ended", () => {
  const step = { type: "tool" as const, itemId: "c1", name: "Bash", detail: "sleep 2", status: "inProgress" as const, throughPosition: 0, item: "command" as const };
  const task = { id: "p1", itemId: "c1", kind: "terminal" as const, label: "sleep 2", status: "completed" as const, startedAtMs: 1000, endedAtMs: 3000, outputFile: null, summary: null, lastLine: null };
  const ended = stepInTurn(step, { end: "done", endedAtMs: 1200 }, task);
  assert.equal(ended?.status, "completed");
  assert.equal(ended?.endedAtMs, 3000);
});

test("a reply with no words is no text of its turn, so the turn's parts and texts count the same (D58)", () => {
  const blocks = cliBlocks(
    {
      entries: [
        user("u1", "Ask me three things", 1000),
        at({ type: "message", itemId: "a1", role: "assistant", text: "1. Colour?\n2. Size?" }, 1100, { phase: "answer" }),
        at({ type: "message", itemId: "a2", role: "assistant", text: "On the card." }, 1200, { phase: "commentary" }),
        at({ type: "message", itemId: "a3", role: "assistant", text: "" }, 1300, { phase: "answer" }),
        at({ type: "message", itemId: "a4", role: "assistant", text: " \n" }, 1350),
        at({ type: "messageDelta", itemId: "a5", text: "  \n" }, 1360),
      ],
      turns: [turn("u1", 1000, 1400, "done")],
    },
    "c",
  );
  assert.deepEqual(blocks[0]!.texts.map((text) => text.messageId), ["a1", "a2"]);
});

/** A block held for its worker, a message the user sent meanwhile woke the session to judge (its turn joins it), its worker's word, then a message after it all. */
const heldTurns = (joinedRequests: string[]): ThreadTurn[] => [
  { ...turn("u1", 1000, 1200, "done"), requests: ["r1"] },
  { ...turn("q1", 3000, 3300, "done"), requests: joinedRequests, joins: true },
  turn("n1", 6000, 6200, "done"),
  { ...turn("u2", 9000, 9200, "done"), requests: ["r2"] },
];

test("a message sent while the block still held for its workers goes on in that block: none of its own while judged, then its bubble where the CLI read it; once the work ended a message starts its own block", () => {
  const id = "0192aaaa-bbbb-7ccc-8ddd-0000feedbeef";
  const words = "Also make it fast.";
  const news = (itemId: string, worker: string, atMs: number) => user(itemId, `[worker events wn-0123456789ab] Worker "Fix" (${worker}): finished · Done. (ev-1234abcd)`, atMs);
  const entries = [
    user("u1", "Fix it with a worker", 1000),
    at({ type: "message", itemId: "a1", role: "assistant", text: "A worker is on it." }, 1100),
    // Its turn over, its worker still working: the user's message woke the session to judge it.
    user("q1", `[queued message feedbeef · judge it: brigadier queue feedbeef join|wait|worker <id>|new-session; unjudged, it joins this work]\n${words}`, 3000),
    at({ type: "command", itemId: "c1", command: "brigadier queue feedbeef join", cwd: null, status: "completed", exitCode: 0, output: "", durationMs: 40 }, 3100),
    at({ type: "message", itemId: "a2", role: "assistant", text: "That goes with the fix." }, 3200),
    news("n1", "aa11bb22", 6000),
    at({ type: "message", itemId: "a3", role: "assistant", text: "Fixed, and fast." }, 6100),
    // Everything over: the user's next message.
    user("u2", "Thanks, now the docs", 9000),
    at({ type: "message", itemId: "a4", role: "assistant", text: "On it." }, 9100),
  ];
  const workers = [row("aa11bb22", 1050)];
  const queued = { id, text: words, attachments: [], mentions: [], queuedAtMs: 2990, editedAtMs: null, deciding: true, delivered: true };

  // Still judged: the rail has it, and the turn it woke goes on in the held block, no block of its own.
  const judging = cliBlocks({ entries, turns: heldTurns([]), workers }, "conv", { ...NO_BOARD, queueItems: [queued] });
  assert.deepEqual(judging.map((block) => [block.key, block.user?.kind === "message" ? block.user.text : null]), [
    ["u1", "Fix it with a worker"],
    ["u2", "Thanks, now the docs"],
  ]);
  assert.deepEqual(judging[0]!.steers, []);
  assert.deepEqual(judging[0]!.texts.map((text) => text.messageId), ["a1", "a2", "a3"]);

  // Joined: its bubble is in the held block where the CLI read it, before the work it led to.
  const joined = cliBlocks({ entries, turns: heldTurns([id]), workers }, "conv", NO_BOARD);
  assert.deepEqual(joined.map((block) => block.key), ["u1", "u2"]);
  const [held, after] = joined;
  assert.deepEqual(held!.steers.map((steer) => [steer.message.id, steer.text]), [[id, words]]);
  assert.ok(held!.steers[0]!.position < held!.texts[1]!.position);
  assert.deepEqual(held!.requestIds, ["r1", id]);
  // One block of work: from its first message to its last turn, the wait for its worker between.
  assert.equal(held!.state, "done");
  assert.equal(held!.startedAtMs, 1000);
  assert.deepEqual(held!.worked, [{ fromMs: 1000, toMs: 1200 }, { fromMs: 3000, toMs: 3300 }, { fromMs: 6000, toMs: 6200 }]);
  // The worker's word still joins the block that started it.
  assert.deepEqual(held!.taskNotices.map((notice) => notice.inWork), [true]);
  // The message after the work ended starts its own block.
  assert.equal(after!.user?.kind === "message" && after!.user.text, "Thanks, now the docs");
  assert.deepEqual(after!.steers, []);
});
