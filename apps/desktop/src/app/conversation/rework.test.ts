import assert from "node:assert/strict";
import { test } from "node:test";

import { reworkableRequest } from "@/app/conversation/rework";
import type { OrchestratorStep, UserRequest } from "@/ipc/generated";
import type { Board } from "@/state/board";

const request = (id: string, startedAtMs: number) =>
  ({ id, startedAtMs, state: { type: "done" } }) as unknown as UserRequest;

const merged = (askedIn: string | null, requestId: string | null): OrchestratorStep => ({
  requestId,
  kind: { type: "merged", branch: "brigadier/flow", base: "main", commits: 1, askedIn },
  atMs: 3,
  position: 0,
});

const board = (orchestratorSteps: OrchestratorStep[]) =>
  ({
    requests: { r1: request("r1", 1), r2: request("r2", 2) },
    tasks: {},
    approvals: {},
    orchestratorSteps,
  }) as unknown as Board;

test("a request whose words merged the session can no longer be edited or answered again", () => {
  assert.equal(reworkableRequest(board([])), "r2");
  assert.equal(reworkableRequest(board([merged("r2", null)])), null);
  assert.equal(reworkableRequest(board([merged(null, "r2")])), null);
});

test("a merge another request asked for leaves the latest request reworkable", () => {
  assert.equal(reworkableRequest(board([merged("r1", "r1")])), "r2");
});
