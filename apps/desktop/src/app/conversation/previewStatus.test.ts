import assert from "node:assert/strict";
import { test } from "node:test";

import { latestLine, previewIndicator, previewActions, previewActive, previewStateLabel, previewSummary, previewWord } from "@/app/conversation/previewStatus";
import type { EventEnvelope, Preview, PreviewState } from "@/ipc/generated";
import { applyToBoard, emptyBoard } from "@/state/board";

test("states show their exit code or stop reason and only live previews have controls", () => {
  const states: PreviewState[] = [{ type: "starting" }, { type: "running" }, { type: "paused" }, { type: "exited", code: 2, status: "exit 2" }, { type: "stopped", reason: "the session closed" }];
  assert.deepEqual(states.map(previewActive), [true, true, true, false, false]);
  assert.deepEqual(states.map(previewStateLabel), ["Starting", "Running", "Paused", "Exited · code 2", "Stopped · the session closed"]);
  assert.equal(previewStateLabel({ type: "exited", code: null, status: "killed by signal 9" }), "Exited · killed by signal 9");
  for (const platform of ["macos", "linux"])
    assert.deepEqual(states.map((state) => previewActions(state, platform)), [["Pause", "Stop"], ["Pause", "Stop"], ["Resume", "Stop"], [], []]);
  for (const platform of ["windows", ""])
    assert.deepEqual(states.map((state) => previewActions(state, platform)), [["Stop"], ["Stop"], ["Stop"], [], []]);
});

test("the latest line drops escape codes and progress-bar redraws", () => {
  assert.equal(latestLine("   \u001b[1m\u001b[32mCompiling\u001b[0m brigadier-core v0.1.0\n"), "Compiling brigadier-core v0.1.0");
  assert.equal(latestLine("Compiling a\n    Building [=====>  ] 120/300: a\r    Building [======> ] 121/300: brigadier-core\r\u001b[K\n\n"), "Building [======> ] 121/300: brigadier-core");
  assert.equal(latestLine("\u001b]0;title\u0007Ready\r\n"), "Ready");
  assert.equal(latestLine(""), "");
});

const preview = (id: string, state: PreviewState): Preview => ({ id, conversationId: "c", name: id, command: "pnpm dev", workdir: "/w", workspace: "/w", pid: 1, state, startedAtMs: 0, endedAtMs: null, log: null, kind: "web", building: false });
const event = (seq: number, body: EventEnvelope["event"]) => ({ event: body, streamSeq: seq, atMs: seq, seq, stream: "conversation:c" }) as EventEnvelope;

test("Clear finished keeps starting, running and paused previews", () => {
  const states: PreviewState[] = [{ type: "starting" }, { type: "running" }, { type: "paused" }, { type: "exited", code: 0, status: "exit 0" }];
  let board = emptyBoard("c");
  states.forEach((state, at) => { board = applyToBoard(board, event(at + 1, { type: "previewUpdated", preview: preview(`preview-${at + 1}`, state) })); });
  board = applyToBoard(board, event(9, { type: "previewsCleared", conversationId: "c", ids: Object.keys(board.previews) }));
  assert.deepEqual(Object.keys(board.previews).toSorted(), ["preview-1", "preview-2", "preview-3"]);
});

const web = (state: PreviewState, building = false) => ({ ...preview("page", state), building });
const app = (state: PreviewState) => ({ ...preview("app", state), kind: "app" as const });
test("an activity's state word: building, starting, running for an app, showing for a page, stopped or failed", () => {
  assert.equal(previewWord(web({ type: "starting" }, true)), "Building");
  assert.equal(previewWord(web({ type: "starting" })), "Starting");
  assert.equal(previewWord(web({ type: "running" })), "Showing");
  assert.equal(previewWord(app({ type: "running" })), "Running");
  assert.equal(previewWord(web({ type: "paused" })), "Paused");
  assert.equal(previewWord(web({ type: "exited", code: 0, status: "exit 0" })), "Stopped");
  assert.equal(previewWord(web({ type: "stopped", reason: "by the user" } as PreviewState)), "Stopped");
  assert.equal(previewWord(web({ type: "exited", code: 1, status: "exit 1" })), "Failed");
  assert.equal(previewIndicator({ type: "exited", code: null, status: "signal 9" }), "failed");
});

test("preview states map onto the standalone agent-status states", () => {
  assert.deepEqual([
    { type: "starting" }, { type: "running" }, { type: "paused" },
    { type: "exited", code: 0, status: "exit 0" }, { type: "stopped", reason: "session closed" },
    { type: "exited", code: 1, status: "exit 1" },
  ].map((state) => previewIndicator(state as PreviewState)), ["working", "working", "waiting", "done", "done", "failed"]);
});

test("the inbox counts every preview state without calling finished work ready", () => {
  assert.equal(previewSummary([]), "");
  assert.equal(previewSummary([
    preview("a", { type: "starting" }), preview("b", { type: "running" }),
    preview("c", { type: "paused" }), preview("d", { type: "exited", code: 0, status: "exit 0" }),
    preview("e", { type: "exited", code: 1, status: "exit 1" }),
    preview("f", { type: "exited", code: null, status: "signal 9" }),
    preview("g", { type: "stopped", reason: "session closed" }),
  ]), "1 starting · 1 running · 1 paused · 1 finished · 2 failed · 1 stopped");
});
