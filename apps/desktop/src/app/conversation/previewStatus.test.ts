import assert from "node:assert/strict";
import { test } from "node:test";

import { previewActions, previewActive, previewStateLabel } from "@/app/conversation/previewStatus";
import type { PreviewState } from "@/ipc/generated";

test("states show their exit code or stop reason and only live previews have controls", () => {
  const states: PreviewState[] = [{ type: "running" }, { type: "paused" }, { type: "exited", code: 2, status: "exit 2" }, { type: "stopped", reason: "the session closed" }];
  assert.deepEqual(states.map(previewActive), [true, true, false, false]);
  assert.deepEqual(states.map(previewStateLabel), ["Running", "Paused", "Exited · code 2", "Stopped · the session closed"]);
  assert.equal(previewStateLabel({ type: "exited", code: null, status: "killed by signal 9" }), "Exited · killed by signal 9");
  for (const platform of ["macos", "linux"])
    assert.deepEqual(states.map((state) => previewActions(state, platform)), [["Pause", "Stop"], ["Resume", "Stop"], [], []]);
  for (const platform of ["windows", ""])
    assert.deepEqual(states.map((state) => previewActions(state, platform)), [["Stop"], ["Stop"], [], []]);
});
