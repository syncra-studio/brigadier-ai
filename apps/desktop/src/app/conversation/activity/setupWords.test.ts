import assert from "node:assert/strict";
import { test } from "node:test";

import { setupRows, setupStarting, setupStepWords } from "@/app/conversation/activity/setupWords";
import { blockSequence, type BoardDigest } from "@/app/conversation/blocks";
import { cliBlocks, cliThreadTree } from "@/app/conversation/cliBlocks";
import type { EventEnvelope, Message, RunState, StartStepRecord } from "@/ipc/generated";
import { applyToBoard, emptyBoard } from "@/state/board";

const record = (step: StartStepRecord["step"], position: number, error: string | null = null): StartStepRecord =>
  ({ step, error, requestId: "m0", position });

test("a setup line has a row per step reached, the last running while the turn starts", () => {
  const steps = [record("fetching", 2), record("worktree", 3), record("setupScript", 4), record("setupScript", 5, "exit 1"), record("cli", 6)];
  assert.deepEqual(setupRows(steps, true).map((row) => [row.step, row.status]), [
    ["fetching", "done"], ["worktree", "done"], ["setupScript", "failed"], ["cli", "running"],
  ]);
  assert.deepEqual(setupRows(steps, false).at(-1)?.status, "done");
  // A worktree that couldn't be made fails its row, with git's words.
  const failed = setupRows([record("fetching", 2), record("worktree", 3), record("worktree", 4, "fatal: already checked out")], false);
  assert.deepEqual(failed.map((row) => [row.step, row.status, row.error]), [["fetching", "done", null], ["worktree", "failed", "fatal: already checked out"]]);
  const names = { base: "main", branch: "brigadier/fix", cli: "Claude Code" };
  assert.equal(setupStepWords("worktree", "done", names), "Created brigadier/fix from main");
  assert.equal(setupStepWords("cli", "running", names), "Starting Claude Code");
});

test("the setup line comes first in its turn, before the CLI's items, and stays once it ran", () => {
  const view = {
    entries: [
      { event: { type: "message", itemId: "u1", role: "user", text: "Fix it" }, atMs: 1000 },
      { event: { type: "message", itemId: "a1", role: "assistant", text: "Done." }, atMs: 2000 },
    ],
    turns: [{ userMessage: "u1", startedAtMs: 1000, endedAtMs: 3000, end: "done", error: null, requests: ["m0"] }],
  } as const;
  const board: BoardDigest = { tasks: {}, questions: {}, orchestratorSteps: [], machineSteps: [], decisions: [], compactions: {},
    startSteps: [record("worktree", 3), record("cli", 4)] };
  const [block] = cliBlocks(view as never, "conv", board, true);
  const sequence = blockSequence(block as never);
  assert.equal(sequence[0]?.kind === "orchestrator" && sequence[0].steps[0]?.kind.type, "setup");
});

test("a turn still starting, or whose start failed, shows its setup line before the CLI has it", () => {
  const message: Message = { id: "m0", conversationId: "conv", seq: 1, role: "user", text: "Fix it", blob: null, createdAtMs: 5000,
    attachments: [], mentions: [], model: null, requestId: "m0", parentId: null };
  const board: BoardDigest = { tasks: {}, questions: {}, orchestratorSteps: [], machineSteps: [], decisions: [], compactions: {},
    startSteps: [record("fetching", 2), record("worktree", 3), record("worktree", 4, "fatal: no")] };
  for (const run of [{ state: "starting", error: null }, { state: "failed", error: "fatal: no" }] as const) {
    const tree = cliThreadTree({ entries: [], turns: [] }, "conv", [message], {}, [], board, run as never, true, 0);
    const block = tree.nodes.find((node) => node.kind === "block")?.block;
    assert.equal(block?.orchestratorSteps[0]?.kind.type, "setup", run.state);
  }
});

const runState = (state: RunState, streamSeq: number, step?: StartStepRecord["step"]): EventEnvelope =>
  ({ seq: streamSeq, stream: "conversation:c", streamSeq, atMs: streamSeq,
    event: { type: "runStateChanged", conversationId: "c", state, error: null, requestId: "m0", ...(step && { step }) } });

test("a relaunch mid-turn (a switch) never starts the setup line again", () => {
  let board = emptyBoard("c");
  for (const envelope of [runState("starting", 3), runState("starting", 4, "fetching"), runState("starting", 5, "worktree"), runState("starting", 6, "cli")]) board = applyToBoard(board, envelope);
  assert.equal(setupStarting(board), true);
  board = applyToBoard(board, runState("running", 8));
  assert.equal(setupStarting(board), false);
  // The CLI started again on another account, the same request still working.
  board = applyToBoard(board, runState("starting", 13));
  assert.equal(setupStarting(board), false);
});
