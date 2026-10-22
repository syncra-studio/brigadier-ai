import assert from "node:assert/strict";
import { test } from "node:test";

import { stripWords, workersStrip } from "@/app/conversation/activity/stripView";
import type { WorkerRow } from "@/ipc/generated";

const NOW = 1_000_000;
let next = 0;
const row = (patch: Partial<WorkerRow> = {}): WorkerRow => {
  next += 1;
  return {
    id: `t${next}`, number: next, title: `Worker ${next}`, agent: "claude", model: null, startedAtMs: next * 10,
    working: true, lastEnd: null, gone: false, lastEventAtMs: NOW, limit: null, freshContext: false, waiting: [], mcp: [],
    branch: null, base: null, diff: null, ahead: 0, merged: false, ...patch,
  };
};

test("the strip lists only the workers at work or waiting, oldest first; finished ones are the context card's", () => {
  const working = row();
  const asks = row({ waiting: [{ kind: "permission", what: "rm -rf build", card: null }] });
  const finished = [row({ working: false, lastEnd: "done" }), row({ working: false, lastEnd: "stopped" }), row({ working: false, lastEnd: "failed", gone: true }), row({ working: false, lastEnd: "done", merged: true, ahead: 1 })];
  const view = workersStrip([asks, ...finished, working], NOW);
  assert.deepEqual(view?.rows, [working.id, asks.id]);
  assert.equal(view?.working, 1);
  assert.equal(view?.waiting, 1);
  assert.equal(stripWords(view!), "1 worker working · 1 waiting");
});

test("with no worker at work the strip is gone", () => {
  assert.equal(workersStrip([], NOW), null);
  assert.equal(workersStrip([row({ working: false, lastEnd: "done" }), row({ working: false, lastEnd: "stopped" })], NOW), null);
});

test("Stop all shows only while a worker's CLI still runs", () => {
  assert.equal(workersStrip([row()], NOW)?.stoppable, true);
  // Its CLI gone, its limit still holding: nothing to stop.
  assert.equal(workersStrip([row({ gone: true, limit: { kind: "usageWindow", resetsAtMs: NOW + 1, window: null } })], NOW)?.stoppable, false);
});

test("the collapsed strip counts in one line, naming workers once", () => {
  assert.equal(stripWords({ working: 2, waiting: 0 }), "2 workers working");
  assert.equal(stripWords({ working: 1, waiting: 1 }), "1 worker working · 1 waiting");
  assert.equal(stripWords({ working: 0, waiting: 3 }), "3 workers waiting");
});
