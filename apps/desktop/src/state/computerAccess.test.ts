import assert from "node:assert/strict";
import { test } from "node:test";

import type { ComputerAccess } from "@/ipc/generated";
import { WATCH_ALLOWED_MS, WATCH_MS, watchEvery } from "./computerAccess";

test("the permission rows read often while one is missing or the helper restarts, and keep reading once both are in", () => {
  const both: ComputerAccess = { available: true, accessibility: true, screenRecording: true, restarting: false, problem: null };
  assert.equal(watchEvery(null), WATCH_MS);
  assert.equal(watchEvery({ ...both, accessibility: false }), WATCH_MS);
  assert.equal(watchEvery({ ...both, screenRecording: false }), WATCH_MS);
  assert.equal(watchEvery({ ...both, restarting: true }), WATCH_MS);
  assert.equal(watchEvery(both), WATCH_ALLOWED_MS, "one may still be taken away in System Settings");
  assert.equal(watchEvery({ ...both, available: false }), null);
});
