import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("an overnight run is the session's one Job card in every state, with Start, Edit, Stop (which asks) and Merge", { timeout: 60000 }, async (t) => {
  const { columns, tabs, steps } = JSON.parse(await renderFixtureLive(t, "run-card.html", "run-card-result", 30000, { reducedMotion: true }));

  // One card for the run, under the context card: no card of its own, no wait for its end.
  for (const id of ["proposed", "running", "finished", "failed", "stopped", "partial", "restricted", "bare", "planning"]) {
    assert.deepEqual(columns[id].cards, ["context", "job"], id);
    assert.equal(columns[id].run, id, id);
    assert.equal(columns[id].schedule, null, id);
  }

  // Before the start: its phases one per line, nothing filled, when the report is ready, one
  // warning, Start and Edit.
  assert.deepEqual(columns.proposed, {
    ...columns.proposed,
    state: "proposed",
    title: "Clean up the importer",
    meta: "3 phases",
    segments: ["pending", "pending", "pending"],
    fills: ["", "", ""],
    line: ["pending:Split the importer", "pending:Move retry loop", "pending:Drop the old path"],
    detail: ["Report ready at 7:30"],
    warning: "On battery: plug in to be safe",
    stop: false,
    buttons: ["Start", "Edit"],
  });

  // At work: the phase at work filled as far as its workers got, before the next; when it wraps up; Stop.
  assert.equal(columns.running.title, "Clean up the importer · 2 of 3");
  assert.deepEqual(columns.running.segments, ["done", "active", "pending"]);
  assert.deepEqual(columns.running.fills, ["100%", "40%", ""]);
  assert.deepEqual(columns.running.line, ["active:Move retry loop", "next:Drop the old path"]);
  assert.match(columns.running.detail[0], /^Wraps up at 7:30/);
  assert.equal(columns.running.stop, true);

  // A usage limit: the same card, and the limit's own wait under it, the only wait card left.
  assert.deepEqual(columns.quota.cards, ["context", "job", "schedule"]);
  assert.equal(columns.quota.run, "quota");
  assert.match(columns.quota.schedule, /^2 workers resume at 3:00/);

  // Done in full: all green, what came of it, the time it took, and Merge.
  assert.deepEqual(columns.finished, {
    ...columns.finished,
    state: "success",
    title: "Clean up the importer",
    meta: "7h 12m",
    fills: ["100%", "100%", "100%"],
    line: [],
    outcome: "All three phases are done and checked.",
    detail: ["Not merged yet · 1 thing waits on you"],
    stop: false,
    buttons: ["Merge"],
  });

  // A failed phase in red, its words under it; no Stop, no Merge in the card.
  assert.deepEqual(columns.failed, {
    ...columns.failed,
    state: "failed",
    title: "Phase 2 failed",
    meta: "4h 05m",
    segments: ["done", "failed", "pending"],
    line: ["failed:Move retry loop", "next:Drop the old path"],
    detail: ["Tests kept failing. The report says why."],
    stop: false,
    buttons: [],
  });

  // Stopped: the phase it stopped in grey as far as it got.
  assert.equal(columns.stopped.state, "cancelled");
  assert.equal(columns.stopped.title, "Stopped at phase 2");
  assert.equal(columns.stopped.meta, "3h 12m");
  assert.deepEqual(columns.stopped.segments, ["done", "pending", "pending"]);
  assert.equal(columns.stopped.fills[1], "28.5714%");
  assert.deepEqual(columns.stopped.detail, ["You stopped it. The work so far is saved."]);
  assert.deepEqual(columns.stopped.buttons, []);

  // A phase partly done keeps its word, and its time ran out.
  assert.equal(columns.partial.state, "partial");
  assert.equal(columns.partial.title, "Phase 2 partly done");
  assert.deepEqual(columns.partial.segments, ["done", "partial", "pending"]);
  assert.deepEqual(columns.partial.detail, ["The retry test is left."]);

  // A phase the restrictions skip is no step of the run.
  assert.equal(columns.restricted.title, "Clean up the importer · 2 of 2");
  assert.deepEqual(columns.restricted.line, ["active:Drop the old path"]);

  // A bare goal: writing the plan is its one step, before the start and while the run works.
  assert.deepEqual(columns.bare.line, ["pending:Write the plan"]);
  assert.deepEqual(columns.bare.detail, ["Runs until done"]);
  assert.deepEqual(columns.bare.buttons, ["Start", "Edit"]);
  assert.deepEqual(columns.planning.line, ["active:Write the plan"]);
  assert.deepEqual(columns.planning.detail, ["Writing the plan"]);

  // A merged run gives the card back to the session's own work.
  assert.equal(columns.change.run, null);
  assert.equal(columns.change.title, "Building");

  // The job's tab: each phase's scope and "done when" before the start; the report once over,
  // with Merge.
  assert.equal(tabs.proposed.parts, 1);
  assert.equal(tabs.proposed.scopes.length, 3);
  assert.match(tabs.proposed.scopes[1], /^Move the retry loop into the reader, behind one setting\.Done when/);
  assert.match(tabs.finished.report, /The importer is ready to merge\./);
  assert.deepEqual(tabs.finished.buttons, ["Merge", "Show in the thread"]);

  assert.equal(steps.error, undefined, JSON.stringify(steps));
  // The live card: each button sends its command and never opens the tab; Stop asks first.
  assert.deepEqual(steps.start.methods, ["startOvernight"]);
  assert.equal(steps.start.tab, null);
  assert.equal(steps.start.view.state, "running");
  assert.equal(steps.ask.question, "Stop the overnight run?");
  assert.deepEqual(steps.ask.methods, ["startOvernight"]);
  assert.equal(steps.ask.tab, null);
  assert.deepEqual(steps.stop.methods, ["startOvernight", "stopOvernight"]);
  assert.equal(steps.stop.tab, null);
  assert.equal(steps.stop.view.stop, false);
  assert.deepEqual(steps.merge.methods, ["startOvernight", "stopOvernight", "mergeOvernight"]);
  assert.equal(steps.merge.verifiedCommit, "def5678abc");
  assert.equal(steps.merge.tab, null);
  // A click on the card itself, and Edit, open the job's tab.
  assert.equal(steps.open.tab, "panel:reviews");
  assert.equal(steps.edit.tab, "panel:reviews");
});
