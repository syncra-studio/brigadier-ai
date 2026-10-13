import assert from "node:assert/strict";
import { test } from "node:test";

import { latestLine, previewChip, previewActions, previewActive, previewStateLabel } from "@/app/conversation/previewStatus";
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

const preview = (id: string, state: PreviewState): Preview => ({ id, conversationId: "c", name: id, command: "pnpm dev", workdir: "/w", workspace: "/w", pid: 1, state, startedAtMs: 0, endedAtMs: null, log: null, kind: "web" });
const event = (seq: number, body: EventEnvelope["event"]) => ({ event: body, streamSeq: seq, atMs: seq, seq, stream: "conversation:c" }) as EventEnvelope;

test("Clear finished keeps starting, running and paused previews", () => {
  const states: PreviewState[] = [{ type: "starting" }, { type: "running" }, { type: "paused" }, { type: "exited", code: 0, status: "exit 0" }];
  let board = emptyBoard("c");
  states.forEach((state, at) => { board = applyToBoard(board, event(at + 1, { type: "previewUpdated", preview: preview(`preview-${at + 1}`, state) })); });
  board = applyToBoard(board, event(9, { type: "previewsCleared", conversationId: "c", ids: Object.keys(board.previews) }));
  assert.deepEqual(Object.keys(board.previews).toSorted(), ["preview-1", "preview-2", "preview-3"]);
});

test("the pill names the newest active preview, then the newest finished one", () => {
  const starting = { ...preview("build", { type: "starting" }), startedAtMs: 2 };
  const running = preview("server", { type: "running" });
  const finished = { ...preview("old", { type: "exited", code: 0, status: "exit 0" }), startedAtMs: 3 };
  const chip = previewChip({ starting, running, finished });
  assert.equal(chip?.label, "build");
  assert.equal(chip?.status, "Starting");
  assert.equal(chip?.active, true);
  assert.equal(previewChip({ running })?.status, "Running");
  assert.equal(previewChip({ paused: preview("paused", { type: "paused" }) })?.status, "Paused");
  assert.equal(previewChip({ finished })?.status, "Finished");
  assert.equal(previewChip({ finished })?.active, false);
  assert.equal(previewChip({}), null);
});
