import assert from "node:assert/strict";
import { test } from "node:test";

import { previewActions, previewActive, previewChip, previewStateLabel } from "@/app/conversation/previewStatus";
import type { Preview, PreviewState } from "@/ipc/generated";

const preview = (id: string, name: string, startedAtMs: number, state: PreviewState): Preview => ({
  id, conversationId: "c1", name, command: `npm run ${name}`, workdir: "/wt/session", workspace: "/wt/session",
  pid: 42, state, startedAtMs, endedAtMs: null, log: null,
});
const byId = (...previews: Preview[]) => Object.fromEntries(previews.map((p) => [p.id, p]));

test("the chip keeps paused and finished previews reachable", () => {
  assert.equal(previewChip({}), null);
  assert.equal(previewChip(byId(preview("1", "web", 1, { type: "paused" })))?.status, "Paused");
  assert.equal(previewChip(byId(preview("1", "web", 1, { type: "exited", code: 1, status: "exit 1" })))?.status, "Finished");
});

test("the chip names active previews and lists their commands newest first", () => {
  assert.deepEqual(previewChip(byId(
    preview("1", "old", 0, { type: "stopped", reason: "user" }),
    preview("2", "api", 1, { type: "running" }), preview("3", "web", 2, { type: "paused" }),
  )), { label: "2 previews", status: "Running", title: "web: npm run web (in /wt/session)\napi: npm run api (in /wt/session)" });
});

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
