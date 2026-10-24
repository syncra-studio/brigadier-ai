import assert from "node:assert/strict";
import { test } from "node:test";

import type { UserRequest } from "@/ipc/generated";
import { emptyBoard } from "@/state/board";
import { pendingPaths } from "@/state/pendingChanges";

const turn = (id: string, startedAtMs: number, review: UserRequest["review"]): UserRequest => ({
  id, startedAtMs, review,
  changes: { settled: true, stat: { files: [{ path: "src/file.ts", insertions: 1, deletions: 0, binary: false }], insertions: 1, deletions: 0 } },
} as UserRequest);

test("snapshot turns do not imply pending review, including after an older pending turn (issue 528)", () => {
  const board = emptyBoard("chat");
  board.requests = { r1: turn("r1", 1, {}), r2: turn("r2", 2, {}) };
  assert.deepEqual(pendingPaths(board, "chat"), []);
  board.requests.r1!.review = { "src/file.ts": "pending" };
  assert.deepEqual(pendingPaths(board, "chat"), []);
  delete board.requests.r2;
  assert.deepEqual(pendingPaths(board, "chat"), ["src/file.ts"]);
  assert.deepEqual(pendingPaths(board, "other"), []);
  board.requests.r1!.undo = { reverted: true, commits: [] };
  assert.deepEqual(pendingPaths(board, "chat"), []);
});
