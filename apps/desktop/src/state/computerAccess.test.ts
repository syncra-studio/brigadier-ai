import assert from "node:assert/strict";
import { test } from "node:test";

import type { ComputerAccess } from "@/ipc/generated";
import { watchingComputerAccess } from "./computerAccess";

test("the permission rows keep reading while one is missing or the helper restarts, and stop once both are in", () => {
  const both: ComputerAccess = { available: true, accessibility: true, screenRecording: true, restarting: false, problem: null };
  assert.equal(watchingComputerAccess(null), true);
  assert.equal(watchingComputerAccess({ ...both, accessibility: false }), true);
  assert.equal(watchingComputerAccess({ ...both, screenRecording: false }), true);
  assert.equal(watchingComputerAccess({ ...both, restarting: true }), true);
  assert.equal(watchingComputerAccess(both), false);
});
