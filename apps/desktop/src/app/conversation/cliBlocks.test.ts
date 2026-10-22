import assert from "node:assert/strict";
import { test } from "node:test";

import { cliBlocks, cliThreadTree, exitOf, stepDetail, toolItem } from "@/app/conversation/cliBlocks";
import { INTERRUPTED } from "@/app/conversation/turnError";
import type { BoardDigest } from "@/app/conversation/blocks";
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
const NO_BOARD: BoardDigest = { tasks: {}, approvals: {}, questions: {}, plans: {}, orchestratorSteps: [], machineSteps: [], decisions: [], compactions: {} };

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
  assert.equal(toolItem("mcp__brigadier__run", null), "command");
  assert.equal(toolItem("Read", '{"file_path":"shot.PNG"}'), "image");
  assert.equal(toolItem("WebFetch", null), "web");
  assert.equal(toolItem("Write", null), "files");
  assert.equal(stepDetail('{"action":"findInPage","pattern":"lts","url":"https://nodejs.org"}'), "'lts' in https://nodejs.org");
  assert.equal(stepDetail('{"description":"x"}'), null);
  assert.equal(exitOf("Exit code 2\nboom"), 2);
  assert.equal(exitOf('[{"type":"text","text":"[exit 1] failed"}]'), 1);
  assert.equal(exitOf("fine"), undefined);
});
