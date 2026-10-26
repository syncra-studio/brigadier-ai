import assert from "node:assert/strict";
import { test } from "node:test";

import { stripWords, subagentsStrip, subagentWords, workersStrip } from "@/app/conversation/activity/stripView";
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

/** One of the session CLI's own agents, as `workersOf` makes its row. */
const native = (patch: Partial<WorkerRow> = {}, backgroundId: string | null = "bg") => {
  const base = row(patch);
  return { ...base, native: { conversationId: "c", callId: base.id, backgroundId, prompt: "" } };
};

test("the session's own subagents leave the Workers strip for the Subagents strip", () => {
  const worker = row();
  const agent = native();
  assert.deepEqual(workersStrip([worker, agent], NOW)?.rows, [worker.id]);
  assert.equal(workersStrip([agent], NOW), null);
  assert.deepEqual(subagentsStrip([worker, agent], NOW)?.rows, [agent.id]);
});

test("finished subagents leave immediately while another works, and remain in their source records", () => {
  const working = native();
  const done = native({ working: false, lastEnd: "done", gone: true, lastEventAtMs: NOW - 20_000 });
  const failed = native({ working: false, lastEnd: "failed", gone: true, lastEventAtMs: NOW - 3_000 });
  const old = native({ working: false, lastEnd: "done", gone: true, lastEventAtMs: NOW - 40_000 });
  const view = subagentsStrip([working, done, failed, old], NOW);
  assert.deepEqual(view?.rows, [working.id]);
  assert.equal(subagentWords(view!), "1 subagent working");
  assert.deepEqual(view?.stops, ["bg"]);
  // Nothing at work: still there just after the last end, then gone.
  assert.equal(subagentsStrip([done, failed], NOW), null);
  assert.equal(subagentsStrip([done, old], NOW), null);
  assert.equal(subagentsStrip([], NOW), null);
});

test("Stop all stops only the subagents the CLI can stop", () => {
  assert.deepEqual(subagentsStrip([native({}, null)], NOW)?.stops, []);
});
